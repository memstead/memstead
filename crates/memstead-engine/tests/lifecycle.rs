//! Integration tests for the full mem-lifecycle orchestrators
//! (`create_mem`, `delete_mem`). These tests construct an
//! `memstead_base::Engine` directly from folder mounts and exercise the
//! orchestrators against it — no MCP, no CLI, no workspace store.
//!
//! Imported wholesale from `memstead-base/src/engine/lifecycle.rs` when the
//! orchestrators moved to this crate. The test bodies are unchanged
//! beyond the import-path rewrites; the on-disk shapes and assertion
//! invariants are exactly what lean ran before the lift.

use std::path::PathBuf;

use memstead_base::backend::MemBackend;
use memstead_base::storage::FilesystemMemWriter;
use memstead_base::workspace::{
    CreateRuleSetting, DeleteRuleSetting, Mount, MountCapability, MountLifecycle, MountStorage,
    WorkspaceSettings,
};
use memstead_base::{Engine, EngineError};
use memstead_engine::{FullEngineError, mem_management};
use memstead_schema::SchemaRef;
use tempfile::TempDir;

fn pin(name: &str) -> SchemaRef {
    let pin_str = match name {
        "default" => "default@1.0.0".to_string(),
        other => format!("{other}@0.1.0"),
    };
    pin_str.parse().expect("static test pin parses")
}

fn folder_mount(mem: &str, path: PathBuf) -> Mount {
    Mount {
        mem: mem.to_string(),
        schema: Some(pin("default")),
        storage: MountStorage::Folder { path },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    }
}

#[test]
fn create_mem_rejects_overlong_note() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            location: tmp.path().join("alpha"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: Some("x".repeat(mem_management::NOTE_MAX_LEN + 1)),
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::Lean(EngineError::InvalidInput(msg)) => {
            assert!(msg.contains("note exceeds"))
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn create_mem_rejects_when_no_allowlist_configured() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            location: tmp.path().join("alpha"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::MemPathNotAllowed { reason, .. } => {
            assert_eq!(reason, "no_allowlist_configured");
        }
        other => panic!("expected MemPathNotAllowed, got {other:?}"),
    }
}

#[test]
fn create_mem_rejects_name_collision() {
    // Engine boots with mem "alpha"; trying to create another
    // "alpha" surfaces MemNameCollision before any disk write.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("alpha");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("alpha", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    // Wildcard allowlist so the allowlist check doesn't bounce us first.
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "*".to_string(),
            schemas: vec!["*".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            location: tmp.path().join("alpha"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::Lean(EngineError::MemNameCollision { name, .. }) => {
            assert_eq!(name, "alpha")
        }
        other => panic!("expected MemNameCollision, got {other:?}"),
    }
}

#[test]
fn create_mem_succeeds_with_wildcard_rule() {
    // Wildcard allowlist + matching basename → mem is created,
    // registered, and its config.json is on disk.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "*".to_string(),
            schemas: vec!["*".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let new_loc = tmp.path().join("alpha");
    let response = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            location: new_loc.clone(),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: Some("seed".to_string()),
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap();

    assert_eq!(response.name, "alpha");
    // Folder backends produce a non-empty synthetic commit id;
    // git-branch backends produce a 40-char hex sha. Either way,
    // the cursor is non-empty.
    assert!(
        !response.seed_commit_sha.is_empty(),
        "seed_commit_sha should be a non-empty cursor (synthetic for folder backend, real sha for git-branch)",
    );
    // Mem is now visible + writable.
    assert!(engine.mem_router().is_writable("alpha"));
    // config.json landed on disk.
    let config_path = new_loc.join(".memstead").join("config.json");
    assert!(
        config_path.exists(),
        "config.json must land at {config_path:?}"
    );
}

/// The optional `write_guidance` create-parameter is persisted
/// verbatim into the new mem's config `writeGuidance` map in the
/// seed commit (schema-strictness D8 — the engine never inspects the
/// keys). A populated map round-trips; the `camelCase` wire key is
/// `writeGuidance`.
#[test]
fn create_mem_persists_write_guidance_into_seed_config() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "*".to_string(),
            schemas: vec!["*".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let mut guidance = std::collections::HashMap::new();
    guidance.insert(
        "phase_context".to_string(),
        serde_json::json!("early design"),
    );
    guidance.insert("stack".to_string(), serde_json::json!("Rust"));

    let new_loc = tmp.path().join("alpha");
    mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: guidance,
            name: "alpha".to_string(),
            vcs: None,
            location: new_loc.clone(),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: Some("seed".to_string()),
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap();

    let config_path = new_loc.join(".memstead").join("config.json");
    let bytes = std::fs::read(&config_path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["writeGuidance"]["phase_context"], "early design",
        "seed config must carry the opaque write_guidance verbatim under the camelCase key; got {value}",
    );
    assert_eq!(value["writeGuidance"]["stack"], "Rust");
}

/// Hierarchical paths are first-class. `params.name = "planning/alpha"`
/// matches a `planning/*` rule against the full name verbatim — no
/// separate `<path>/<name>` composition step. Confirms the allowlist
/// candidate IS the mem name.
#[test]
fn create_mem_with_hierarchical_name_matches_path_rule() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "planning/*".to_string(),
            schemas: vec!["*".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let new_loc = tmp.path().join("alpha");
    let response = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "planning/alpha".to_string(),
            vcs: None,
            location: new_loc.clone(),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap();

    assert_eq!(response.name, "planning/alpha");
    assert!(!response.seed_commit_sha.is_empty());
    // The full hierarchical path IS the router key.
    assert!(engine.mem_router().is_writable("planning/alpha"));
    assert!(!engine.mem_router().is_writable("alpha"));
}

/// The mem-name grammar refuses leading-underscore segments
/// (reserved for registry refs like `__MEMSTEAD`). Validation fires
/// before any disk side effect.
#[test]
fn create_mem_rejects_double_underscore_segment() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "**".to_string(),
            schemas: vec!["*".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "__reserved/alpha".to_string(),
            vcs: None,
            location: tmp.path().join("alpha"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap_err();
    match err {
        // Reserved-prefix names route through the typed
        // `InvalidMemName` variant with the `reserved_prefix` reason
        // discriminator, not the `InvalidInput` catch-all.
        FullEngineError::InvalidMemName { name, reason } => {
            assert_eq!(name, "__reserved/alpha");
            assert_eq!(reason, "reserved_prefix");
        }
        other => panic!("expected InvalidMemName, got {other:?}"),
    }
}

/// The structural-failure matrix. Four distinct invalid inputs surface
/// four distinct typed codes, rather than collapsing to
/// `MEM_PATH_NOT_ALLOWED (no_match)` — which would make an empty-name
/// typo indistinguishable from a legitimate allowlist refusal.
#[test]
fn create_mem_structural_invalid_name_matrix() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    // Permissive allowlist — the four cases should refuse with the
    // typed code before the allowlist gate fires.
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "**".to_string(),
            schemas: vec!["*".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let cases: &[(&str, &str)] = &[
        ("", "empty"),
        ("other with spaces", "whitespace"),
        ("__reserved", "reserved_prefix"),
        // Non-printable ASCII character — fails the regex grammar
        // and routes through the `invalid_char` fallback.
        ("bad\u{0007}name", "invalid_char"),
    ];

    for (name, expected_reason) in cases {
        let err = mem_management::create_mem(
            &mut engine,
            mem_management::MemCreateParams {
                write_guidance: Default::default(),
                name: (*name).to_string(),
                vcs: None,
                location: tmp.path().join("any"),
                schema_ref: "default@1.0.0".parse().unwrap(),
                note: None,
                operator_mode: false,
                recovery: None,
                storage: None,
            },
        )
        .unwrap_err();
        match err {
            FullEngineError::InvalidMemName {
                name: got_name,
                reason,
            } => {
                assert_eq!(got_name, *name, "name echoes the offending input");
                assert_eq!(
                    reason, *expected_reason,
                    "reason discriminator drifted for input {name:?}"
                );
            }
            other => panic!(
                "input {name:?} expected InvalidMemName (reason={expected_reason}), got {other:?}"
            ),
        }
    }
}

#[test]
fn create_mem_rejects_basename_mismatch() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "*".to_string(),
            schemas: vec!["*".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            // basename is "beta" — mismatch with name "alpha".
            location: tmp.path().join("beta"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::Lean(EngineError::InvalidInput(msg)) => {
            assert!(msg.contains("does not match the basename"));
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

/// Explicit `storage: Some(GitBranch)` in a folder-shaped workspace
/// (no `<workspace_root>/mem-repo/.git/`) refuses with a typed
/// `InvalidInput` — there is no gitdir to host the branch.
#[test]
fn create_mem_rejects_explicit_git_branch_without_mem_repo() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            location: tmp.path().join("alpha"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: true,
            recovery: None,
            storage: Some(mem_management::StorageKind::GitBranch),
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::Lean(EngineError::InvalidInput(msg)) => {
            assert!(
                msg.contains("mem-repo/.git"),
                "refusal must name the missing mem-repo/.git, got: {msg}"
            );
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn delete_mem_rejects_when_no_allowlist_configured() {
    // Engine with no [[mem_management.delete]] rules → V1
    // unified always errors with MemPathNotAllowed.
    // Mirrors full's agent-mode contract; operator_mode bypass
    // lifts later.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "specs".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::MemPathNotAllowed {
            reason, candidate, ..
        } => {
            assert_eq!(reason, "no_allowlist_configured");
            assert_eq!(candidate, "specs");
        }
        other => panic!("expected MemPathNotAllowed, got {other:?}"),
    }
    // Engine state unchanged — mem still writable.
    assert!(engine.mem_router().is_writable("specs"));
}

#[test]
fn delete_mem_rejects_unknown_name() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "missing".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        FullEngineError::Lean(EngineError::UnknownMem(v)) if v == "missing"
    ));
}

#[test]
fn delete_mem_rejects_overlong_note() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let long_note = "x".repeat(mem_management::NOTE_MAX_LEN + 1);
    let err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "specs".to_string(),
            delete_files: false,
            note: Some(long_note),
            operator_mode: false,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::Lean(EngineError::InvalidInput(msg)) => {
            assert!(msg.contains("note exceeds"))
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn delete_mem_unregisters_when_allowlist_matches() {
    // Build an engine with a delete-rule that admits the mem.
    // Call delete_mem; expect the mem unregistered + files
    // not deleted (delete_files = false).
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "specs".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "specs".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap();
    assert_eq!(response.name, "specs");
    assert!(response.deleted_from_router);
    assert!(!response.files_deleted);

    // Engine state: mem unregistered, directory still on disk.
    assert!(!engine.mem_router().is_writable("specs"));
    assert!(mem_dir.exists(), "delete_files=false leaves directory");
}

/// Round-trip: a flat-layout namespace (`exec-*`) admits both
/// create and delete. The create's bare-name candidate matches the
/// `exec-*` create rule; the delete reads the registered
/// `mem_path` (None for flat) and composes the same bare-name
/// candidate against the `exec-*` delete rule.
#[test]
fn create_delete_round_trip_flat_namespace() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "exec-*".to_string(),
            schemas: vec!["default@1.0.0".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "exec-*".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let new_loc = tmp.path().join("exec-foo");
    mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "exec-foo".to_string(),
            vcs: None,
            location: new_loc.clone(),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap();
    assert!(engine.mem_router().is_writable("exec-foo"));

    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "exec-foo".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap();
    assert!(response.deleted_from_router);
    assert!(!engine.mem_router().is_writable("exec-foo"));
}

/// Round-trip: a hierarchical namespace (`planning/plan-*`) admits
/// both create and delete. Hierarchical paths are first-class —
/// `params.name = "planning/plan-foo"` IS the full identifier (no
/// separate `params.path` composition). The router HashMap key, the
/// lifecycle-allowlist candidate, and `is_writable()` lookups all
/// converge on the same string.
#[test]
fn create_delete_round_trip_hierarchical_namespace() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "planning/plan-*".to_string(),
            schemas: vec!["default@1.0.0".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "planning/plan-*".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let new_loc = tmp.path().join("plan-foo");
    mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "planning/plan-foo".to_string(),
            vcs: None,
            location: new_loc.clone(),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap();
    // The router carries the full hierarchical path as the key —
    // leaf-only lookups miss.
    assert!(engine.mem_router().is_writable("planning/plan-foo"));
    assert!(!engine.mem_router().is_writable("plan-foo"));

    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "planning/plan-foo".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap();
    assert!(response.deleted_from_router);
    assert!(!engine.mem_router().is_writable("planning/plan-foo"));
}

/// A hierarchical create followed by a delete whose rule list omits
/// the hierarchical pattern surfaces `MEM_PATH_NOT_ALLOWED` with the
/// full mem name as the candidate. Hierarchical paths are the
/// identifier —
/// the agent reads `planning/plan-foo` directly without any
/// composition step.
#[test]
fn delete_mem_hierarchical_name_in_path_not_allowed_envelope() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "planning/plan-*".to_string(),
            schemas: vec!["default@1.0.0".to_string()],
            default_cross_links: None,
        }],
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "plan-*".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "planning/plan-foo".to_string(),
            vcs: None,
            location: tmp.path().join("plan-foo"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap();

    let err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "planning/plan-foo".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::MemPathNotAllowed {
            reason, candidate, ..
        } => {
            assert_eq!(reason, "no_match");
            // The candidate IS the mem name verbatim — no
            // composition step.
            assert_eq!(candidate, "planning/plan-foo");
        }
        other => panic!("expected MemPathNotAllowed, got {other:?}"),
    }
    assert!(engine.mem_router().is_writable("planning/plan-foo"));
}

#[test]
fn delete_mem_with_delete_files_removes_directory() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "*".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "specs".to_string(),
            delete_files: true,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap();
    assert!(response.deleted_from_router);
    assert!(response.files_deleted);
    assert!(!mem_dir.exists(), "delete_files=true removes directory");
}

// ---------------------------------------------------------------------------
// Item 01 — operator-mode bypass for mem-lifecycle gates
// ---------------------------------------------------------------------------

/// `operator_mode = true` on `create_mem` admits a path/name
/// combination against a workspace whose `[[mem_management.create]]`
/// list is empty. The same call with `operator_mode = false` returns
/// `MEM_PATH_NOT_ALLOWED` reason=`no_allowlist_configured` —
/// confirming the bypass is scoped to operator-mode and agent-mode
/// behaviour is unchanged.
#[test]
fn create_mem_operator_mode_bypasses_empty_allowlist() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    // Zero allowlist rules — agent-mode rejects every candidate.
    engine.set_settings(WorkspaceSettings::default());

    // Agent-mode rejects with the no_allowlist_configured envelope.
    let agent_err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            location: tmp.path().join("alpha"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: false,
            recovery: None,
            storage: None,
        },
    )
    .unwrap_err();
    match agent_err {
        FullEngineError::MemPathNotAllowed { reason, .. } => {
            assert_eq!(reason, "no_allowlist_configured");
        }
        other => panic!("expected MemPathNotAllowed, got {other:?}"),
    }

    // Operator-mode admits the same call against the same workspace.
    let response = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            location: tmp.path().join("alpha"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: None,
            operator_mode: true,
            recovery: None,
            storage: None,
        },
    )
    .unwrap();
    assert_eq!(response.name, "alpha");
    assert!(engine.mem_router().is_writable("alpha"));
}

/// `operator_mode = true` does NOT bypass safety-shaped checks. An
/// over-long note still surfaces `INVALID_INPUT`, mirroring agent-mode
/// behaviour — operator-mode is a policy bypass, not a safety bypass.
#[test]
fn create_mem_operator_mode_still_enforces_input_validation() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings::default());

    let err = mem_management::create_mem(
        &mut engine,
        mem_management::MemCreateParams {
            write_guidance: Default::default(),
            name: "alpha".to_string(),
            vcs: None,
            location: tmp.path().join("alpha"),
            schema_ref: "default@1.0.0".parse().unwrap(),
            note: Some("x".repeat(mem_management::NOTE_MAX_LEN + 1)),
            operator_mode: true,
            recovery: None,
            storage: None,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::Lean(EngineError::InvalidInput(msg)) => {
            assert!(msg.contains("note exceeds"));
        }
        other => panic!("expected InvalidInput from operator-mode, got {other:?}"),
    }
}

/// `operator_mode = true` on `delete_mem` admits an unregister
/// against a workspace whose `[[mem_management.delete]]` list is
/// empty. Agent-mode against the same workspace returns
/// `MEM_PATH_NOT_ALLOWED` reason=`no_allowlist_configured`.
#[test]
fn delete_mem_operator_mode_bypasses_empty_allowlist() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    // Zero delete rules — agent-mode rejects every candidate.
    engine.set_settings(WorkspaceSettings::default());

    // Agent-mode rejects.
    let agent_err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "specs".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap_err();
    match agent_err {
        FullEngineError::MemPathNotAllowed { reason, .. } => {
            assert_eq!(reason, "no_allowlist_configured");
        }
        other => panic!("expected MemPathNotAllowed, got {other:?}"),
    }

    // Operator-mode admits the same unregister against the same workspace.
    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "specs".to_string(),
            delete_files: false,
            note: None,
            operator_mode: true,
        },
    )
    .unwrap();
    assert!(response.deleted_from_router);
    assert!(!engine.mem_router().is_writable("specs"));
}

/// The `MEM_REFERENCED_BY_POLICY` check gates on `delete_files: true`
/// instead of `!operator_mode`. The safeguard protects against orphaning a
/// grant by destroying the storage it relies on, so router-only
/// unregister (`delete_files: false`) is admitted regardless of
/// mode (grants survive against the preserved storage and re-
/// activate on re-init), while full destruction
/// (`delete_files: true`) refuses in both modes — operators are
/// required to revoke the grant explicitly before destroying the
/// data.
#[test]
fn delete_mem_policy_check_gates_on_delete_files() {
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp = TempDir::new().unwrap();
    let target_dir = tmp.path().join("target");
    let referrer_dir = tmp.path().join("referrer");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::create_dir_all(&referrer_dir).unwrap();

    let target_writer = FilesystemMemWriter::new(target_dir.clone());
    let referrer_writer = FilesystemMemWriter::new(referrer_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("target", target_dir.clone()),
            Box::new(target_writer) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("referrer", referrer_dir),
            Box::new(referrer_writer) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let mut cross_links: std::collections::BTreeMap<String, CrossLinkValue> = Default::default();
    cross_links.insert(
        "referrer".to_string(),
        CrossLinkValue::List(vec!["target".to_string()]),
    );
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "*".to_string(),
        }],
        cross_mem_links: cross_links,
        ..Default::default()
    });

    // Agent-mode + delete_files: true → refused (storage destruction
    // would orphan the grant).
    let agent_destroy_err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "target".to_string(),
            delete_files: true,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap_err();
    match agent_destroy_err {
        FullEngineError::MemReferencedByPolicy {
            name,
            referring_mems,
        } => {
            assert_eq!(name, "target");
            assert_eq!(referring_mems, vec!["referrer".to_string()]);
        }
        other => {
            panic!("expected MemReferencedByPolicy for agent-mode delete_files=true, got {other:?}")
        }
    }

    // Operator-mode + delete_files: true → STILL refused. Operator-
    // mode bypasses the allowlist (access control) but not the
    // integrity check (data destruction would still orphan the
    // grant). The hard stop forces a revoke-first flow.
    let operator_destroy_err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "target".to_string(),
            delete_files: true,
            note: None,
            operator_mode: true,
        },
    )
    .unwrap_err();
    match operator_destroy_err {
        FullEngineError::MemReferencedByPolicy { name, .. } => {
            assert_eq!(name, "target");
        }
        other => panic!(
            "expected MemReferencedByPolicy for operator-mode delete_files=true, got {other:?}"
        ),
    }

    // Agent-mode + delete_files: false → ADMITTED. The grant
    // survives against the preserved storage.
    let agent_unregister = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "target".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap();
    assert!(agent_unregister.deleted_from_router);
    assert!(!engine.mem_router().is_writable("target"));
}

// ---------------------------------------------------------------------------
// Mem-delete referrer scan. Revoking a grant closes the workspace-
// policy axis but not the edge-graph axis. Without this check, a mem
// whose grant was revoked but whose surviving Write-Mem peers still
// carry cross-mem edges into it deletes cleanly, leaving dangling
// references that resolve to nothing.
// ---------------------------------------------------------------------------

/// Positive complement. A delete where the doomed mem has cross-mem
/// Write-Mem incoming edges refuses with the typed
/// `MEM_HAS_INCOMING_REFS` envelope listing every offending source.
#[test]
fn delete_mem_refuses_when_cross_mem_incoming_edges_remain() {
    use memstead_base::ops::RelateArg;
    use memstead_base::vcs::Actor;
    use memstead_base::{CreateEntityArgs, RelateEntityArgs};
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp = TempDir::new().unwrap();
    let a_dir = tmp.path().join("mem-a");
    let b_dir = tmp.path().join("mem-b");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    let a_writer = FilesystemMemWriter::new(a_dir.clone());
    let b_writer = FilesystemMemWriter::new(b_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("a", a_dir.clone()),
            Box::new(a_writer) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("b", b_dir.clone()),
            Box::new(b_writer) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    // Allow `a → b` cross-mem links so the relate call below
    // succeeds. The grant is revoked before the delete attempt so
    // the policy axis (Step 3, `MEM_REFERENCED_BY_POLICY`) passes
    // while the edge-graph axis (Step 3a) fires — the exact
    // F15 / CLI F8 scenario.
    let mut cross_links: std::collections::BTreeMap<String, CrossLinkValue> = Default::default();
    cross_links.insert("a".to_string(), CrossLinkValue::List(vec!["b".to_string()]));
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "b".to_string(),
        }],
        cross_mem_links: cross_links,
        ..Default::default()
    });

    // Seed entities: a--source, b--target. The engine refuses on
    // missing required sections, so seed `identity` + `purpose` on
    // both entities via a tiny CreateEntityArgs sections-builder. The
    // sections type (`IndexMap<String, String>`) is structurally the
    // same across the workspace; we build it through a sample
    // CreateEntityArgs so this test target doesn't need the indexmap
    // crate as a direct dev-dep.
    let seed_sections = || {
        let mut sample = CreateEntityArgs {
            anchors: Vec::new(),
            mem: String::new(),
            title: String::new(),
            entity_type: String::new(),
            sections: Default::default(),
            metadata: Default::default(),
            relations: Vec::new(),
            dry_run: false,
        };
        sample
            .sections
            .insert("identity".to_string(), "seed identity".to_string());
        sample
            .sections
            .insert("purpose".to_string(), "seed purpose".to_string());
        sample.sections
    };
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "b".to_string(),
                title: "Target".to_string(),
                entity_type: "spec".to_string(),
                sections: seed_sections(),
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("seed b--target");
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "a".to_string(),
                title: "Source".to_string(),
                entity_type: "spec".to_string(),
                sections: seed_sections(),
                metadata: Default::default(),
                relations: vec![RelateArg {
                    to: memstead_base::EntityId::canonical("b--target"),
                    rel_type: "DEPENDS_ON".to_string(),
                    description: None,
                }],
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("seed a--source with DEPENDS_ON edge");

    // Revoke the cross-mem grant — Step 3 now passes — leaving
    // the edge state as the F15 scenario: policy is clean but the
    // edge graph still has the stale reference.
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "b".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    // Mem-delete refuses with the typed envelope.
    let err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "b".to_string(),
            delete_files: true,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::Lean(EngineError::MemHasIncomingRefs { mem, referrers }) => {
            assert_eq!(mem, "b");
            assert_eq!(referrers.len(), 1, "exactly one Write-Mem referrer");
            assert_eq!(referrers[0].from_id, "a--source");
            assert_eq!(referrers[0].mem, "a");
            assert_eq!(referrers[0].rel_types, vec!["DEPENDS_ON".to_string()]);
        }
        other => panic!("expected MemHasIncomingRefs, got {other:?}"),
    }
    // Mem b still routable — refusal is pre-unregister.
    assert!(engine.mem_router().is_writable("b"));

    // Re-grant the cross-mem link so the relate --remove call
    // passes the policy gate. Real-world the operator either reverts
    // the revoke, or removes edges via `memstead_update`-section-drop
    // which doesn't require a policy grant. Either way, this test
    // verifies the post-cleanup delete path.
    let mut cross_links: std::collections::BTreeMap<String, CrossLinkValue> = Default::default();
    cross_links.insert("a".to_string(), CrossLinkValue::List(vec!["b".to_string()]));
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "b".to_string(),
        }],
        cross_mem_links: cross_links,
        ..Default::default()
    });
    // Remove the edge; retry the delete; succeeds.
    engine
        .relate_entity(
            RelateEntityArgs {
                source: memstead_base::EntityId::canonical("a--source"),
                target: memstead_base::EntityId::canonical("b--target"),
                rel_type: "DEPENDS_ON".to_string(),
                description: None,
                expected_hash: None,
                remove: true,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("relate --remove");
    // Revoke again so Step 3 passes on retry.
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "b".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "b".to_string(),
            delete_files: true,
            note: None,
            operator_mode: false,
        },
    )
    .expect("delete succeeds after edge removed");
    assert!(response.deleted_from_router);
    assert!(!engine.mem_router().is_writable("b"));

    // Post-delete dangling-link scan confirms a--source has no
    // dangling references.
    let dangling = memstead_base::ops::health::collect_dangling_links(engine.store(), None);
    assert!(
        dangling.is_empty(),
        "post-delete state must be coherent; got dangling = {dangling:?}"
    );
}

/// The `MEM_HAS_INCOMING_REFS` gate fires unconditional — a router-only
/// unregister (`delete_files: false`) with stale cross-mem edges
/// refuses with the same envelope a destructive delete would have
/// raised. The gate is not wrapped in `if params.delete_files {`, so
/// unregister-only deletes against mems whose surviving writable peers
/// still point at them are refused rather than silently admitted.
#[test]
fn delete_mem_router_only_refuses_when_cross_mem_incoming_edges_remain() {
    use memstead_base::CreateEntityArgs;
    use memstead_base::ops::RelateArg;
    use memstead_base::vcs::Actor;
    use memstead_schema::workspace_config::CrossLinkValue;

    let tmp = TempDir::new().unwrap();
    let a_dir = tmp.path().join("mem-a");
    let b_dir = tmp.path().join("mem-b");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    let a_writer = FilesystemMemWriter::new(a_dir.clone());
    let b_writer = FilesystemMemWriter::new(b_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("a", a_dir.clone()),
            Box::new(a_writer) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("b", b_dir.clone()),
            Box::new(b_writer) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    let mut cross_links: std::collections::BTreeMap<String, CrossLinkValue> = Default::default();
    cross_links.insert("a".to_string(), CrossLinkValue::List(vec!["b".to_string()]));
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "b".to_string(),
        }],
        cross_mem_links: cross_links,
        ..Default::default()
    });

    let seed_sections = || {
        let mut sample = CreateEntityArgs {
            anchors: Vec::new(),
            mem: String::new(),
            title: String::new(),
            entity_type: String::new(),
            sections: Default::default(),
            metadata: Default::default(),
            relations: Vec::new(),
            dry_run: false,
        };
        sample
            .sections
            .insert("identity".to_string(), "seed identity".to_string());
        sample
            .sections
            .insert("purpose".to_string(), "seed purpose".to_string());
        sample.sections
    };
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "b".to_string(),
                title: "Target".to_string(),
                entity_type: "spec".to_string(),
                sections: seed_sections(),
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("seed b--target");
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "a".to_string(),
                title: "Source".to_string(),
                entity_type: "spec".to_string(),
                sections: seed_sections(),
                metadata: Default::default(),
                relations: vec![RelateArg {
                    to: memstead_base::EntityId::canonical("b--target"),
                    rel_type: "DEPENDS_ON".to_string(),
                    description: None,
                }],
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("seed a--source with DEPENDS_ON edge");

    // Router-only unregister (`delete_files: false`) refuses
    // when the edge graph still carries a cross-mem reference into
    // the doomed mem. The policy axis (Step 3) is gated on
    // `delete_files: true`, but the edge axis (Step 3a) is split out
    // to fire unconditionally.
    let err = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "b".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .unwrap_err();
    match err {
        FullEngineError::Lean(EngineError::MemHasIncomingRefs { mem, referrers }) => {
            assert_eq!(mem, "b");
            assert_eq!(referrers.len(), 1);
            assert_eq!(referrers[0].from_id, "a--source");
            assert_eq!(referrers[0].mem, "a");
            assert_eq!(referrers[0].rel_types, vec!["DEPENDS_ON".to_string()]);
        }
        other => panic!("expected MemHasIncomingRefs, got {other:?}"),
    }
    // Mem b still routable — refusal is pre-unregister.
    assert!(engine.mem_router().is_writable("b"));
}

/// A successful destructive delete
/// (`delete_files: true`) atomically scrubs the workspace.toml of the
/// now-dangling `[cross_mem_links]` grants naming the deleted mem
/// — but PRESERVES the `[[mem_management.create]]` /
/// `[[mem_management.delete]]` allowlist rules, exact-name ones
/// included, since those are forward-looking permissions for the name.
/// The engine's in-memory settings refresh from the freshly-edited
/// file so a follow-up agent doesn't see a stale cross-link grant.
#[test]
fn destructive_delete_scrubs_cross_links_but_keeps_allowlist_rules() {
    use memstead_base::CreateEntityArgs;
    use memstead_base::vcs::Actor;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    let memstead = workspace.join(".memstead");
    std::fs::create_dir_all(&memstead).unwrap();
    // The doomed mem `other` grants `test` as a write target (key
    // match — scrubbed). A neutral `[cross_mem_links]` entry from
    // `test` grants `keep` as a value alongside the doomed mem to
    // verify the scrub drops only `other` and preserves the rest.
    // Pre-scrub the policy gate is empty because no surviving mem
    // grants `other` AS A TARGET; the scrub still wipes the
    // `other = [..]` key plus any value occurrences.
    std::fs::write(
        memstead.join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n\
         [cross_mem_links]\n\
         other = [\"test\"]\n\
         keep = [\"other\", \"survive\"]\n\
         \n\
         [[mem_management.create]]\n\
         pattern = \"other\"\n\
         schemas = [\"default@1.0.0\"]\n\
         \n\
         [[mem_management.create]]\n\
         pattern = \"*\"\n\
         schemas = [\"default@1.0.0\"]\n\
         \n\
         [[mem_management.delete]]\n\
         pattern = \"other\"\n",
    )
    .unwrap();

    let other_dir = workspace.join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("other", other_dir.clone()),
        Box::new(FilesystemMemWriter::new(other_dir)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(workspace.clone());
    // Use empty in-memory cross-link policy so the destructive-delete
    // policy gate passes regardless of the on-disk `[cross_mem_links]`
    // entries (which we want the scrub to see and clear). The on-disk
    // file is the source of truth that the scrub reads and rewrites;
    // the engine refreshes its settings post-scrub from the same file.
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "other".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    // Seed a content entity so the delete pipeline has something to
    // walk. Sections are built via a sample `CreateEntityArgs` to
    // avoid taking `indexmap` as a direct dev-dep of this test target.
    let mut sample = CreateEntityArgs {
        anchors: Vec::new(),
        mem: String::new(),
        title: String::new(),
        entity_type: String::new(),
        sections: Default::default(),
        metadata: Default::default(),
        relations: Vec::new(),
        dry_run: false,
    };
    sample
        .sections
        .insert("identity".to_string(), "seed identity".to_string());
    sample
        .sections
        .insert("purpose".to_string(), "seed purpose".to_string());
    engine
        .create_entity(
            CreateEntityArgs {
                anchors: Vec::new(),
                mem: "other".to_string(),
                title: "Solo".to_string(),
                entity_type: "spec".to_string(),
                sections: sample.sections,
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
            },
            Actor::Cli,
            None,
            None,
        )
        .expect("seed other--solo");

    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "other".to_string(),
            delete_files: true,
            note: None,
            operator_mode: false,
        },
    )
    .expect("destructive delete succeeds");
    assert!(response.deleted_from_router);

    let after = std::fs::read_to_string(memstead.join("workspace.toml")).unwrap();
    assert!(
        !after.contains("\nother = ["),
        "`other` key must be scrubbed from cross_mem_links — got:\n{after}"
    );
    assert!(
        after.contains("\"survive\""),
        "non-target cross-link values must survive — got:\n{after}"
    );
    // The exact-name `pattern = "other"` rules in BOTH
    // `[[mem_management.create]]` and `.delete]]` survive — deleting
    // the instance must not revoke the forward-looking permission to
    // re-create a mem of the same name.
    assert_eq!(
        after.matches("pattern = \"other\"").count(),
        2,
        "exact-name mem_management.{{create,delete}} rules for `other` must be preserved — got:\n{after}"
    );
    assert!(
        after.contains("pattern = \"*\""),
        "wildcard rule must survive — got:\n{after}"
    );
    // In-memory settings must reflect the scrub so a follow-up agent
    // sees the fresh policy state without an explicit reload.
    assert!(
        !engine.settings().cross_mem_links.contains_key("other"),
        "post-scrub in-memory settings still surface the dropped key"
    );
    assert!(
        !engine
            .settings()
            .cross_mem_links
            .get("keep")
            .map(|v| matches!(v,
                memstead_schema::workspace_config::CrossLinkValue::List(l) if l.iter().any(|s| s == "other")
            ))
            .unwrap_or(false),
        "post-scrub in-memory settings still surface the dropped value"
    );
}

/// Complement: router-only unregister (`delete_files: false`) does NOT
/// scrub the policy file. The grants stay valid against the tombstoned storage
/// so a later reattach reactivates the cross-mem permissions.
#[test]
fn router_only_unregister_leaves_policy_intact() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    let memstead = workspace.join(".memstead");
    std::fs::create_dir_all(&memstead).unwrap();
    let toml_body = "format = \"memstead-git-branch-2\"\n\n\
         [cross_mem_links]\n\
         other = [\"test\"]\n";
    std::fs::write(memstead.join("workspace.toml"), toml_body).unwrap();

    let other_dir = workspace.join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("other", other_dir.clone()),
        Box::new(FilesystemMemWriter::new(other_dir)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(workspace.clone());
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "other".to_string(),
        }],
        ..Default::default()
    });

    let _ = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "other".to_string(),
            delete_files: false,
            note: None,
            operator_mode: false,
        },
    )
    .expect("router-only unregister succeeds");

    let after = std::fs::read_to_string(memstead.join("workspace.toml")).unwrap();
    assert_eq!(
        after, toml_body,
        "unregister-only must leave the policy file byte-identical"
    );
}

/// Policy-mutation cache refresh. The end-to-end chain the MCP
/// wrapper executes: write a workspace.toml mutation, re-parse via
/// [`memstead_base::workspace_store::parse_workspace_settings`], apply
/// the projection via `engine.set_settings`. Post-call, the engine's
/// in-memory `settings()` view matches the on-disk state without an
/// intervening `memstead_reload`. F6 MCP closed.
#[test]
fn policy_mutation_refreshes_engine_settings() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    let memstead = workspace.join(".memstead");
    std::fs::create_dir_all(&memstead).unwrap();
    std::fs::write(
        memstead.join("workspace.toml"),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
a = ["b"]
"#,
    )
    .unwrap();

    let mem_dir = workspace.join("mem");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("v", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    // Seed the engine's in-memory settings from the initial TOML.
    let initial = memstead_base::workspace_store::parse_workspace_settings(&workspace).unwrap();
    engine.set_settings(initial);
    assert!(
        engine.settings().cross_mem_links.contains_key("a"),
        "engine must surface the seed grant; got {:?}",
        engine.settings().cross_mem_links
    );

    // Simulate the policy-mutation tool's effect: revoke the grant
    // by rewriting the file (mirrors `workspace_config_edit::revoke_cross_link`'s
    // disk-level outcome).
    std::fs::write(
        memstead.join("workspace.toml"),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
"#,
    )
    .unwrap();

    // Pre-refresh: stale cache surfaces the revoked grant — the
    // footgun the refresh discipline below guards against.
    assert!(
        engine.settings().cross_mem_links.contains_key("a"),
        "pre-refresh cache must still show the old state — proves the bug existed",
    );

    // Apply the refresh discipline the MCP wrapper now invokes:
    // re-parse + set_settings.
    let refreshed = memstead_base::workspace_store::parse_workspace_settings(&workspace).unwrap();
    engine.set_settings(refreshed);
    assert!(
        engine.settings().cross_mem_links.is_empty(),
        "post-refresh cache must reflect the revoke; got {:?}",
        engine.settings().cross_mem_links
    );
}

/// Negative complement. A mem with no Write-Mem incoming edges
/// deletes cleanly; the edge-scan step doesn't fire false positives.
#[test]
fn delete_mem_succeeds_when_no_cross_mem_incoming_edges() {
    let tmp = TempDir::new().unwrap();
    let a_dir = tmp.path().join("mem-a");
    let b_dir = tmp.path().join("mem-b");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    let a_writer = FilesystemMemWriter::new(a_dir.clone());
    let b_writer = FilesystemMemWriter::new(b_dir.clone());
    let mut engine = Engine::from_mounts(vec![
        (
            folder_mount("a", a_dir.clone()),
            Box::new(a_writer) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("b", b_dir.clone()),
            Box::new(b_writer) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: Vec::new(),
        mem_delete_rules: vec![DeleteRuleSetting {
            pattern: "b".to_string(),
        }],
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    // No entities, no edges — pure metadata mem.
    let response = mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "b".to_string(),
            delete_files: true,
            note: None,
            operator_mode: false,
        },
    )
    .expect("delete with no edges should succeed");
    assert!(response.deleted_from_router);
    assert!(!engine.mem_router().is_writable("b"));
}

// --- rule-derived cross-link grant visibility -------

/// A `default_cross_links`-bearing create rule is enforced lazily AND
/// surfaced in `memstead_overview` — the rule-derived grant appears named under
/// its pattern in `## Lifecycle Namespaces` and as the
/// `cross_mem_links_from_rules` workspace-policy posture, without being
/// materialized into `[cross_mem_links]`.
#[test]
fn rule_derived_cross_link_grant_is_enforced_and_surfaced_in_overview() {
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "scratch".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::List(vec!["seed".to_string()])),
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: Default::default(),
        ..Default::default()
    });

    // Enforcement unchanged: grant direction ALLOWS, reverse DENIES,
    // unrelated pair deny-by-default.
    assert!(engine.cross_mem_link_allowed("scratch", "seed"));
    assert!(!engine.cross_mem_link_allowed("seed", "scratch"));
    assert!(!engine.cross_mem_link_allowed("scratch", "elsewhere"));

    // The explicit table is empty — nothing was materialized.
    assert!(engine.settings().cross_mem_links.is_empty());

    // Policy projection surfaces the rule-derived grant distinctly from
    // any explicit grant.
    let entries = memstead_engine::overview::build_workspace_policy_entries(&engine);
    assert!(
        entries
            .iter()
            .any(|(k, v)| *k == "cross_mem_links_from_rules" && v == "named"),
        "rule-derived grant must surface in the policy projection: {entries:?}",
    );
    assert!(
        !entries.iter().any(|(k, _)| *k == "cross_mem_links"),
        "explicit cross_mem_links must stay absent — nothing materialized: {entries:?}",
    );

    // Rendered overview names the grant under its pattern and carries the
    // posture in the `_policy` flow.
    let out = memstead_engine::overview::compose_overview(
        &mut engine,
        memstead_engine::overview::OverviewArgs {
            include: &[],
            mem: None,
            rebuild: false,
            token_budget: 8000,
            operator_mode: false,
            suppress_lifecycle: false,
        },
        memstead_engine::overview::Surface::Mcp,
    )
    .unwrap();
    assert!(
        out.markdown.contains("Cross-mem links (rule-derived)") && out.markdown.contains("seed"),
        "overview must name the rule-derived cross-link target under the pattern:\n{}",
        out.markdown,
    );
    assert!(
        out.policy_flow
            .as_deref()
            .is_some_and(|f| f.contains("cross_mem_links_from_rules")),
        "policy flow must carry the rule-derived posture: {:?}",
        out.policy_flow,
    );
}

/// An explicit grant and a rule-derived grant coexist as two distinct
/// policy-projection entries — no merge, no double-count. Guards the
/// "distinguish explicit vs rule-derived" decision and the "other policy
/// projection intact" complement.
#[test]
fn explicit_and_rule_derived_cross_links_project_as_distinct_entries() {
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("seed");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemMemWriter::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("seed", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let mut cross_links = std::collections::BTreeMap::new();
    cross_links.insert("seed".to_string(), CrossLinkValue::Wildcard);
    engine.set_settings(WorkspaceSettings {
        mem_create_rules: vec![CreateRuleSetting {
            pattern: "scratch".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::List(vec!["seed".to_string()])),
        }],
        mem_delete_rules: Vec::new(),
        cross_mem_links: cross_links,
        ..Default::default()
    });

    let entries = memstead_engine::overview::build_workspace_policy_entries(&engine);
    assert!(
        entries
            .iter()
            .any(|(k, v)| *k == "cross_mem_links" && v == "wildcard"),
        "explicit grant posture must project unchanged: {entries:?}",
    );
    assert!(
        entries
            .iter()
            .any(|(k, v)| *k == "cross_mem_links_from_rules" && v == "named"),
        "rule-derived grant must project as its own entry: {entries:?}",
    );
}
