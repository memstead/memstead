#![cfg(test)]

use super::*;

/// A title-case body wiki-link refusal carries the
/// slug-form retry under `proposed_slug` (mirroring `INVALID_TITLE`),
/// so an agent that wrote `[[Idempotency]]` finds `idempotency` under
/// the key it already knows.
#[test]
fn invalid_wiki_link_details_carry_proposed_slug_for_title_case() {
    let err = EngineError::InvalidWikiLinkTarget {
        raw: "Idempotency".to_string(),
        suggested: Some("idempotency".to_string()),
        section: "purpose".to_string(),
        link_source: "body_link".to_string(),
        reason: "slugs must be lowercase".to_string(),
    };
    let d = err.details();
    assert_eq!(d["proposed_slug"], "idempotency");
    assert_eq!(d["suggested"], "idempotency");
}

/// `SCHEMA_NOT_FOUND` carries the fixed-order resolution
/// diagnostics under `details.sources`: a right-name/wrong-version
/// pin shows the built-in's available versions with
/// `pinned_version_match = false`, and `remote` is the reserved
/// `not_configured` slot. This is the agent-visible payload that
/// tells the caller the name resolves but the version does not.
#[test]
fn schema_not_found_details_carry_fixed_order_source_diagnostics() {
    let requested: semver::Version = "99.0.0".parse().unwrap();
    let sources = SchemaSourceDiagnostic::for_failed_pin("default", &requested, &[]);
    let err = EngineError::SchemaNotFound {
        mem: "specs".to_string(),
        pin: "default@99.0.0".to_string(),
        sources,
        install_hint: None,
    };
    assert_eq!(err.code(), "SCHEMA_NOT_FOUND");
    let d = err.details();
    assert_eq!(d["mem"], "specs");
    assert_eq!(d["pin"], "default@99.0.0");
    let src = d["sources"].as_array().expect("sources is an array");
    let labels: Vec<&str> = src.iter().map(|s| s["source"].as_str().unwrap()).collect();
    assert_eq!(labels, ["local_storage", "builtin", "remote"]);
    // The `default` builtin exists at 1.0.0 — right name, wrong
    // version: builtin enumerates it but the pin does not match.
    let builtin = &src[1];
    assert!(
        builtin["versions_found"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "1.0.0"),
        "builtin must enumerate default@1.0.0, got {builtin:?}",
    );
    assert_eq!(builtin["pinned_version_match"], false);
    // No local storage was consulted (empty `consulted` slice).
    assert_eq!(src[0]["versions_found"].as_array().unwrap().len(), 0);
    // Remote is the reserved, unenumerated slot.
    assert_eq!(src[2]["status"], "not_configured");
    assert!(
        src[2].get("versions_found").is_some(),
        "remote still ships an (empty) versions_found list",
    );
}

/// The MESSAGE (not just `details`) summarises the source trail,
/// and a right-name/wrong-version failure is distinguishable from
/// a never-installed one without opening `details`. The message
/// names exactly the sources in `sources` — never one that was
/// not searched — and the empty-sources internal miss keeps the
/// bare legacy sentence.
#[test]
fn schema_not_found_message_summarises_trail_and_distinguishes_wrong_version() {
    // Wrong version: the builtin catalogue holds `default@1.0.0`,
    // the pin asks for 99.0.0.
    let requested: semver::Version = "99.0.0".parse().unwrap();
    let wrong_version = EngineError::SchemaNotFound {
        mem: "specs".to_string(),
        pin: "default@99.0.0".to_string(),
        sources: SchemaSourceDiagnostic::for_failed_pin("default", &requested, &[]),
        install_hint: None,
    };
    let msg = wrong_version.to_string();
    assert!(msg.contains("searched local_storage"), "got: {msg}");
    assert!(msg.contains("builtin (holds"), "got: {msg}");
    assert!(msg.contains("remote (not_configured)"), "got: {msg}");
    assert!(
        msg.contains("the pinned version is wrong"),
        "wrong-version case must be named in the message: {msg}"
    );
    assert!(
        msg.contains("memstead mem set-schema specs default@1.3.0"),
        "wrong-version case ends in the concrete repin command: {msg}"
    );

    // Never installed: no source holds any version of the name.
    let never: semver::Version = "1.0.0".parse().unwrap();
    let never_installed = EngineError::SchemaNotFound {
        mem: "specs".to_string(),
        pin: "no-such-schema@1.0.0".to_string(),
        sources: SchemaSourceDiagnostic::for_failed_pin("no-such-schema", &never, &[]),
        install_hint: None,
    };
    let msg2 = never_installed.to_string();
    assert!(
        msg2.contains("nothing for \"no-such-schema\""),
        "never-installed case names the empty sources: {msg2}"
    );
    assert!(
        !msg2.contains("the pinned version is wrong"),
        "never-installed must NOT claim a version mismatch: {msg2}"
    );
    assert!(
        msg2.contains("memstead schema install <package-dir>"),
        "never-installed (no probe) still names the install path: {msg2}"
    );
    assert_ne!(msg, msg2, "the two failures are distinguishable");

    // Internal lookup miss (empty sources): bare legacy sentence,
    // no trail is claimed.
    let internal = EngineError::SchemaNotFound {
        mem: "specs".to_string(),
        pin: "x@1.0.0".to_string(),
        sources: Vec::new(),
        install_hint: None,
    };
    assert_eq!(
        internal.to_string(),
        "mem specs: schema pin \"x@1.0.0\" did not resolve in any schema source",
    );
}

/// The install-hint probe attaches the authoring-package pointer
/// exactly when a loadable package with the pin's name sits in the
/// workspace root while NO source holds any version of the name —
/// and stays silent for a version mismatch (installed at another
/// version), for an absent package, and for a non-`SchemaNotFound`
/// error.
#[test]
fn schema_install_probe_hints_only_for_uninstalled_authoring_package() {
    // Workspace root carrying the memstead-schema `examples/minimal`
    // package (name `recipe`) as an authoring folder.
    let tmp = tempfile::TempDir::new().unwrap();
    let src_pkg = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../memstead-schema/examples/minimal");
    let dst = tmp.path().join("recipe");
    std::fs::create_dir_all(dst.join("types")).unwrap();
    std::fs::copy(src_pkg.join("schema.yaml"), dst.join("schema.yaml")).unwrap();
    for entry in std::fs::read_dir(src_pkg.join("types")).unwrap().flatten() {
        std::fs::copy(entry.path(), dst.join("types").join(entry.file_name())).unwrap();
    }

    let requested: semver::Version = "0.1.0".parse().unwrap();
    let not_found = || EngineError::SchemaNotFound {
        mem: "specs".to_string(),
        pin: "recipe@0.1.0".to_string(),
        sources: SchemaSourceDiagnostic::for_failed_pin("recipe", &requested, &[]),
        install_hint: None,
    };

    // Uninstalled + authored → hint attaches, message + details
    // point at `memstead schema install`.
    let hinted = not_found().with_schema_install_probe(Some(tmp.path()));
    let msg = hinted.to_string();
    assert!(
        msg.contains("memstead schema install"),
        "hint must name the install command: {msg}"
    );
    assert!(msg.contains("recipe"), "hint names the package: {msg}");
    let d = hinted.details();
    assert!(
        d["install_hint"]["command"]
            .as_str()
            .unwrap()
            .starts_with("memstead schema install"),
        "details carry the hint: {d}"
    );

    // No workspace root → no concrete package hint; the message
    // falls back to the generic install path.
    let no_root = not_found().with_schema_install_probe(None);
    let no_root_msg = no_root.to_string();
    assert!(
        no_root_msg.contains("memstead schema install <package-dir>"),
        "generic install path without a probe hit: {no_root_msg}"
    );
    assert!(
        !no_root_msg.contains(&tmp.path().display().to_string()),
        "no concrete package path without a probe hit: {no_root_msg}"
    );

    // No such authoring package → same generic fallback, no
    // concrete path.
    let other_tmp = tempfile::TempDir::new().unwrap();
    let absent = not_found().with_schema_install_probe(Some(other_tmp.path()));
    let absent_msg = absent.to_string();
    assert!(
        absent_msg.contains("memstead schema install <package-dir>"),
        "generic install path when no package exists: {absent_msg}"
    );
    assert!(
        !absent_msg.contains(&other_tmp.path().display().to_string()),
        "no concrete package path when no package exists: {absent_msg}"
    );

    // Version mismatch against an installed package (some source
    // holds versions of the name) → no hint even though the
    // authoring package exists.
    let mismatch_req: semver::Version = "99.0.0".parse().unwrap();
    let mismatch = EngineError::SchemaNotFound {
        mem: "specs".to_string(),
        pin: "default@99.0.0".to_string(),
        sources: SchemaSourceDiagnostic::for_failed_pin("default", &mismatch_req, &[]),
        install_hint: None,
    }
    .with_schema_install_probe(Some(tmp.path()));
    let mismatch_msg = mismatch.to_string();
    assert!(
        !mismatch_msg.contains("schema install"),
        "version mismatch must not hint install: {mismatch_msg}"
    );
    assert!(
        mismatch_msg.contains("memstead mem set-schema specs default@1.3.0"),
        "version mismatch hints version repair instead: {mismatch_msg}"
    );

    // Non-SchemaNotFound errors pass through unchanged.
    let other =
        EngineError::UnknownMem("specs".to_string()).with_schema_install_probe(Some(tmp.path()));
    assert_eq!(other.code(), "UNKNOWN_MEM");
}

/// The ambiguous-grammar case suggests a
/// colon-form (`mem:slug`), which is NOT a bare slug — it must not
/// be promoted to `proposed_slug`.
#[test]
fn invalid_wiki_link_colon_form_suggestion_is_not_a_proposed_slug() {
    let err = EngineError::InvalidWikiLinkTarget {
        raw: "team/sub--thing".to_string(),
        suggested: Some("team/sub:thing".to_string()),
        section: "purpose".to_string(),
        link_source: "body_link".to_string(),
        reason: "ambiguous".to_string(),
    };
    let d = err.details();
    assert!(
        d["proposed_slug"].is_null(),
        "colon-form must not be a proposed_slug: {d}"
    );
    assert_eq!(d["suggested"], "team/sub:thing");
}

/// A bad `--since` cursor is the typed `INVALID_CURSOR`
/// code carrying the untruncated SHA in `details.since`.
#[test]
fn invalid_changes_cursor_code_and_details() {
    let sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let err = EngineError::InvalidChangesCursor {
        mem: "specs".to_string(),
        since: sha.to_string(),
    };
    assert_eq!(err.code(), "INVALID_CURSOR");
    let d = err.details();
    assert_eq!(d["mem"], "specs");
    assert_eq!(
        d["since"], sha,
        "the offending SHA must ride untruncated in details"
    );
}
