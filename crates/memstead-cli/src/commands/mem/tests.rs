#![cfg(test)]

use super::*;
use memstead_base::EngineError;
use memstead_base::FullEngineError;
use memstead_base::ReferrerInfo;
use std::path::PathBuf;

fn lifted_cli_error(err: FullEngineError) -> CliError {
    let any = full_engine_err_to_cli(err);
    any.downcast::<CliError>()
        .expect("full_engine_err_to_cli must lift to a CliError")
}

/// The client-side mem-template consumer surfaces a built-in
/// schema's instance guidance keys when `--write-guidance` is
/// omitted, stays silent when guidance is given, and is silent for a
/// schema that ships no template.
#[test]
fn mem_template_guidance_note_surfaces_builtin_keys() {
    let planning: memstead_schema::SchemaRef = "planning@0.1.0".parse().unwrap();
    let note = mem_template_guidance_note(&planning, false)
        .expect("planning ships a mem-template — a note is due");
    assert!(note.contains("phase_context"), "note names the key: {note}");
    assert!(
        note.contains("--write-guidance"),
        "note tells how to fill: {note}"
    );
    // Operator supplied guidance → nothing to surface.
    assert!(mem_template_guidance_note(&planning, false).is_some());
    assert!(mem_template_guidance_note(&planning, true).is_none());
    // A schema with no mem-template → no note.
    let default_: memstead_schema::SchemaRef = "default@1.0.0".parse().unwrap();
    assert!(mem_template_guidance_note(&default_, false).is_none());
}

/// The CLI's mem command surface does not translate the engine's
/// typed code through a static table — a code added on the engine
/// side reaches the CLI envelope unchanged. Pins the regression
/// where `MEM_HAS_INCOMING_REFS` silently degraded to
/// `VALIDATION_FAILED`.
#[test]
fn mem_has_incoming_refs_keeps_typed_code_and_carries_details() {
    let err = FullEngineError::Engine(EngineError::MemHasIncomingRefs {
        mem: "other".to_string(),
        referrers: vec![ReferrerInfo {
            from_id: "test--source".to_string(),
            rel_types: vec!["USES".to_string()],
            mem: "test".to_string(),
        }],
    });
    let cli = lifted_cli_error(err);
    assert_eq!(cli.code, "MEM_HAS_INCOMING_REFS");
    assert_eq!(cli.kind, ExitKind::Validation);
    let details = cli.details.expect("details must reach the CLI envelope");
    assert_eq!(details["mem"], "other");
    let referrers = details["referrers"].as_array().expect("referrers array");
    assert_eq!(referrers.len(), 1);
    assert_eq!(referrers[0]["from_id"], "test--source");
    assert_eq!(referrers[0]["mem"], "test");
}

/// Lifecycle refusal (a `FullEngineError` lifecycle variant) is
/// promoted through with the same code + structured details the
/// MCP wire ships. `MEM_PATH_NOT_ALLOWED` carries the candidate,
/// the patterns list, and the typed reason discriminator.
#[test]
fn mem_path_not_allowed_carries_structured_details() {
    let err = FullEngineError::MemPathNotAllowed {
        attempted: PathBuf::from("/ws/bogus"),
        candidate: "bogus".to_string(),
        patterns: vec!["specs".to_string(), "team/*".to_string()],
        reason: "no_match",
        policy_table: "mem_management.create",
    };
    let cli = lifted_cli_error(err);
    assert_eq!(cli.code, "MEM_PATH_NOT_ALLOWED");
    assert_eq!(cli.kind, ExitKind::Validation);
    let details = cli.details.expect("details");
    assert_eq!(details["candidate"], "bogus");
    assert_eq!(details["reason"], "no_match");
    assert_eq!(details["patterns"][0], "specs");
    // The `policy_table` disambiguator reaches the CLI envelope.
    assert_eq!(details["policy_table"], "mem_management.create");
    assert_eq!(details["patterns"][1], "team/*");
    // The structured remedy reaches the CLI envelope too — the
    // caller can recover from `details` without parsing prose.
    assert!(
        details["remedy"]["cli"]
            .as_str()
            .expect("remedy.cli present")
            .contains("allow-create"),
        "got: {details}"
    );
}

/// `VALIDATION_FAILED` is not
/// used as the fallback for engine-sourced refusals. A
/// typed lifecycle variant must not degrade to the catch-all.
#[test]
fn lifecycle_refusal_never_degrades_to_validation_failed_token() {
    let cases = [
        FullEngineError::MemPathNotAllowed {
            attempted: PathBuf::from("/x"),
            candidate: "x".to_string(),
            patterns: vec![],
            reason: "no_allowlist_configured",
            policy_table: "mem_management.create",
        },
        FullEngineError::MemReferencedByPolicy {
            name: "x".to_string(),
            referring_mems: vec!["y".to_string()],
        },
        FullEngineError::MemSchemaNotAllowed {
            candidate: "x".to_string(),
            matched_pattern: "p".to_string(),
            requested_schema: "default@1.0.0".to_string(),
            allowed_schemas: vec!["other@1.0.0".to_string()],
        },
        FullEngineError::InvalidMemName {
            name: "BadName".to_string(),
            reason: "invalid_char",
        },
    ];
    for err in cases {
        let cli = lifted_cli_error(err);
        assert_ne!(
            cli.code, "VALIDATION_FAILED",
            "engine-sourced refusal must carry its typed code: got {} with details {:?}",
            cli.code, cli.details,
        );
    }
}

/// Wrapped base-engine errors keep the
/// per-variant exit-kind mapping (`NotFound` → exit 3,
/// `HashMismatch` → exit 4, etc.) by delegating to
/// `CliError::from_engine_op`. The lift doesn't flatten every
/// base-engine variant to `Validation`.
#[test]
fn wrapped_base_engine_error_preserves_per_variant_exit_kind() {
    let err = FullEngineError::Engine(EngineError::NotFound {
        id: "specs--missing".to_string(),
    });
    let cli = lifted_cli_error(err);
    assert_eq!(cli.code, "ENTITY_NOT_FOUND");
    assert_eq!(cli.kind, ExitKind::NotFound);
    assert_eq!(cli.details.as_ref().unwrap()["id"], "specs--missing");
}
