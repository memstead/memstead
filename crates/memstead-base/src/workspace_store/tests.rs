#![cfg(test)]

use super::*;
use memstead_schema::SchemaRef;
use std::io::Write as _;
use tempfile::TempDir;

fn pin(s: &str) -> SchemaRef {
    s.parse().unwrap()
}

fn folder_mount(mem: &str, path: PathBuf) -> Mount {
    Mount {
        mem: mem.to_string(),
        schema: Some(pin("default@1.0.0")),
        storage: MountStorage::Folder { path },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    }
}

fn write_workspace_toml(workspace_root: &Path, body: &str) {
    let path = FileWorkspaceStore::workspace_toml_path(workspace_root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

#[test]
fn load_returns_not_initialised_when_memstead_dir_absent() {
    let tmp = TempDir::new().unwrap();
    let store = FileWorkspaceStore::new();
    let err = store.load(tmp.path()).unwrap_err();
    assert!(matches!(err, StoreError::NotInitialised { .. }));
}

/// The [`parse_workspace_settings`] helper reads only
/// `.memstead/workspace.toml` and returns a fresh `WorkspaceSettings`.
/// MCP-driven policy mutations call this after writing to disk
/// to refresh the engine's in-memory cache without re-loading
/// the engine-managed `mounts.json`.
#[test]
fn parse_workspace_settings_reflects_cross_mem_links_edit() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
team-a = ["team-b"]
"#,
    );
    let settings = super::parse_workspace_settings(tmp.path()).unwrap();
    assert!(
        settings.cross_mem_links.contains_key("team-a"),
        "initial parse must surface the team-a grant; got {:?}",
        settings.cross_mem_links
    );

    // Mutate the file (simulating `workspace_config_edit::revoke_cross_link`).
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
"#,
    );
    let refreshed = super::parse_workspace_settings(tmp.path()).unwrap();
    assert!(
        refreshed.cross_mem_links.is_empty(),
        "refreshed parse must drop the team-a grant; got {:?}",
        refreshed.cross_mem_links
    );
}

/// `parse_workspace_settings` surfaces
/// `[[mem_management.create]]` / `[[mem_management.delete]]`
/// rules so the engine's allowlist gate sees the post-mutation
/// state immediately.
#[test]
fn parse_workspace_settings_reflects_allowlist_edit() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let initial = super::parse_workspace_settings(tmp.path()).unwrap();
    assert!(initial.mem_create_rules.is_empty());

    // Mutate (simulating `workspace_config_edit::add_create_rule`).
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[[mem_management.create]]
pattern = "test-*"
schemas = ["default@1.0.0"]
"#,
    );
    let refreshed = super::parse_workspace_settings(tmp.path()).unwrap();
    assert_eq!(refreshed.mem_create_rules.len(), 1);
    assert_eq!(refreshed.mem_create_rules[0].pattern, "test-*");
}

#[test]
fn load_returns_not_initialised_when_workspace_toml_missing() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".memstead")).unwrap();
    let store = FileWorkspaceStore::new();
    let err = store.load(tmp.path()).unwrap_err();
    assert!(matches!(err, StoreError::NotInitialised { .. }));
}

#[test]
fn load_with_no_mounts_yields_empty_mount_list() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let workspace = store.load(tmp.path()).unwrap();
    assert!(workspace.mounts.is_empty());
}

#[test]
fn load_with_no_mem_management_yields_empty_settings() {
    // Workspace.toml without a `[mem_management]` section
    // produces the default empty settings — mirrors full's
    // behaviour where missing rules mean "no agent-driven
    // mem create / delete allowed".
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let workspace = store.load(tmp.path()).unwrap();
    assert!(workspace.settings.mem_create_rules.is_empty());
    assert!(workspace.settings.mem_delete_rules.is_empty());
    assert!(workspace.settings.cross_mem_links.is_empty());
}

#[test]
fn load_picks_up_cross_mem_links_wildcard_and_list() {
    // [cross_mem_links] is parsed via CrossLinkValue::parse_toml
    // to handle the wildcard ("*") vs allowlist ([...]) shape that
    // serde untagged-enum decode can't express. Both shapes round
    // through to WorkspaceSettings.cross_mem_links.
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
specs = "*"
engine = ["specs", "macos"]
locked = []
"#,
    );
    let store = FileWorkspaceStore::new();
    let workspace = store.load(tmp.path()).unwrap();
    let cvl = &workspace.settings.cross_mem_links;
    assert_eq!(cvl.len(), 3);
    assert_eq!(cvl.get("specs"), Some(&CrossLinkValue::Wildcard));
    assert_eq!(
        cvl.get("engine"),
        Some(&CrossLinkValue::List(vec![
            "specs".to_string(),
            "macos".to_string()
        ]))
    );
    assert_eq!(cvl.get("locked"), Some(&CrossLinkValue::List(vec![])));
}

#[test]
fn load_rejects_cross_mem_links_mixed_wildcard_and_names() {
    // The shared parser rejects `["*", "specs"]` — wildcard must
    // be the sole entry. The schema-crate's typed error lifts via
    // StoreError::Parse so the operator-facing error names the
    // exact key.
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[cross_mem_links]
specs = ["*", "engine"]
"#,
    );
    let store = FileWorkspaceStore::new();
    let err = store.load(tmp.path()).unwrap_err();
    match err {
        StoreError::Parse { message, .. } => {
            assert!(message.contains("[cross_mem_links].specs"));
            assert!(message.contains("wildcard"));
        }
        other => panic!("expected StoreError::Parse, got {other:?}"),
    }
}

#[test]
fn load_picks_up_default_cross_links_on_create_rule() {
    // CreateRule.default_cross_links uses the same CrossLinkValue
    // parser. A rule with `default_cross_links = "*"` lifts to
    // CreateRuleSetting.default_cross_links = Some(Wildcard).
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[[mem_management.create]]
pattern = "exec-*"
schemas = ["default"]
default_cross_links = "*"
"#,
    );
    let store = FileWorkspaceStore::new();
    let workspace = store.load(tmp.path()).unwrap();
    let rule = &workspace.settings.mem_create_rules[0];
    assert_eq!(rule.pattern, "exec-*");
    assert_eq!(rule.default_cross_links, Some(CrossLinkValue::Wildcard));
}

#[test]
fn load_picks_up_mem_management_create_and_delete_rules() {
    // Operator-edited `[mem_management]` section flows through
    // FileWorkspaceStore::load into Workspace.settings; the
    // engine layer then propagates via Engine::set_settings.
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"

[[mem_management.create]]
pattern = "exec-*"
schemas = ["default@1.0.0", "*"]

[[mem_management.create]]
pattern = "scratch-*"
schemas = ["default"]

[[mem_management.delete]]
pattern = "exec-*"
"#,
    );
    let store = FileWorkspaceStore::new();
    let workspace = store.load(tmp.path()).unwrap();
    assert_eq!(workspace.settings.mem_create_rules.len(), 2);
    assert_eq!(workspace.settings.mem_create_rules[0].pattern, "exec-*");
    assert_eq!(
        workspace.settings.mem_create_rules[0].schemas,
        vec!["default@1.0.0".to_string(), "*".to_string()]
    );
    assert_eq!(workspace.settings.mem_create_rules[1].pattern, "scratch-*");
    assert_eq!(workspace.settings.mem_delete_rules.len(), 1);
    assert_eq!(workspace.settings.mem_delete_rules[0].pattern, "exec-*");
}

/// Dual-pin state survives the store round-trip: a mount carrying
/// `migration_target` writes it to `mounts.json` and reads it
/// back; settled mounts' entries stay byte-compatible (the key is
/// skipped when `None`).
#[test]
fn save_state_round_trips_migration_target() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        "\nformat = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );
    let store = FileWorkspaceStore::new();
    let mut migrating = folder_mount("specs", PathBuf::from("/work/mem"));
    migrating.migration_target = Some(pin("mig-b@0.1.0"));
    let settled = folder_mount("other", PathBuf::from("/work/other"));
    let original = Workspace {
        mounts: vec![migrating, settled],
        settings: WorkspaceSettings::default(),
    };
    store.save_state(tmp.path(), &original).unwrap();
    let raw = std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
    assert!(
        raw.contains("mig-b@0.1.0"),
        "migration_target must persist: {raw}"
    );
    assert_eq!(
        raw.matches("migration_target").count(),
        1,
        "settled mounts must omit the key entirely: {raw}"
    );
    let loaded = store.load(tmp.path()).unwrap();
    assert_eq!(loaded.mounts[0].migration_target, Some(pin("mig-b@0.1.0")));
    assert_eq!(loaded.mounts[1].migration_target, None);
}

#[test]
fn save_state_then_load_round_trips_mount_list() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let original = Workspace {
        mounts: vec![
            folder_mount("specs", PathBuf::from("/work/mem")),
            Mount {
                mem: "engine".to_string(),
                schema: Some(pin("default@1.0.0")),
                storage: MountStorage::GitBranch {
                    gitdir: PathBuf::from("/work/mem-repo/.git"),
                    branch: "engine".to_string(),
                },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            },
            Mount {
                mem: "external".to_string(),
                schema: Some(pin("default@1.0.0")),
                storage: MountStorage::Archive {
                    path: PathBuf::from("/deps/external.mem"),
                },
                capability: MountCapability::ReadOnly,
                lifecycle: MountLifecycle::Lazy,
                cross_linkable: false,
                migration_target: None,
            },
        ],
        settings: WorkspaceSettings::default(),
    };
    store.save_state(tmp.path(), &original).unwrap();

    // The mounts.json file lives where we expect.
    assert!(FileWorkspaceStore::mounts_json_path(tmp.path()).is_file());

    let reloaded = store.load(tmp.path()).unwrap();
    assert_eq!(reloaded.mounts.len(), original.mounts.len());
    for (a, b) in reloaded.mounts.iter().zip(original.mounts.iter()) {
        assert_eq!(a.mem, b.mem);
        assert_eq!(a.schema, b.schema);
        assert_eq!(a.capability, b.capability);
        assert_eq!(a.lifecycle, b.lifecycle);
        assert_eq!(a.cross_linkable, b.cross_linkable);
        assert_eq!(a.storage, b.storage);
    }
}

#[test]
fn save_state_round_trips_unset_schema_assertion() {
    // `Mount.schema = None` (no expectation assertion) must survive a
    // mounts.json round-trip and omit the `schema` key on the wire —
    // the authoritative pin lives in the mem's backend config, not
    // in the mount record.
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let original = Workspace {
        mounts: vec![Mount {
            mem: "foreign".to_string(),
            schema: None,
            storage: MountStorage::Folder {
                path: tmp.path().join("foreign"),
            },
            capability: MountCapability::ReadOnly,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        }],
        settings: WorkspaceSettings::default(),
    };
    store.save_state(tmp.path(), &original).unwrap();

    // The wire form omits the `schema` key entirely (skip-on-None).
    let raw = std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
    assert!(
        !raw.contains("\"schema\""),
        "unset schema assertion must omit the key on the wire; got:\n{raw}"
    );

    // And it reloads as `None`.
    let reloaded = store.load(tmp.path()).unwrap();
    assert_eq!(reloaded.mounts.len(), 1);
    assert_eq!(reloaded.mounts[0].schema, None);
}

#[test]
fn save_state_does_not_touch_workspace_toml() {
    let tmp = TempDir::new().unwrap();
    let original_body = r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#;
    write_workspace_toml(tmp.path(), original_body);
    let store = FileWorkspaceStore::new();
    let workspace = Workspace::default();
    store.save_state(tmp.path(), &workspace).unwrap();
    // Operator's TOML untouched.
    let toml_after =
        std::fs::read_to_string(FileWorkspaceStore::workspace_toml_path(tmp.path())).unwrap();
    assert_eq!(toml_after, original_body);
}

#[test]
fn save_state_writes_paths_relative_to_workspace_root() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let workspace = Workspace {
        mounts: vec![
            Mount {
                mem: "engine".to_string(),
                schema: Some(pin("default@1.0.0")),
                storage: MountStorage::GitBranch {
                    gitdir: tmp.path().join("mem-repo").join(".git"),
                    branch: "engine".to_string(),
                },
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            },
            Mount {
                mem: "external".to_string(),
                schema: Some(pin("default@1.0.0")),
                storage: MountStorage::Archive {
                    path: PathBuf::from("/global/cache/external.mem"),
                },
                capability: MountCapability::ReadOnly,
                lifecycle: MountLifecycle::Lazy,
                cross_linkable: false,
                migration_target: None,
            },
        ],
        settings: WorkspaceSettings::default(),
    };
    store.save_state(tmp.path(), &workspace).unwrap();

    let on_disk =
        std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
    // Format bumped to V2.
    assert!(on_disk.contains("\"memstead-mounts-3\""));
    // In-workspace path stored relative — no absolute prefix bake-in.
    assert!(
        on_disk.contains("\"mem-repo/.git\""),
        "expected relative gitdir, got: {on_disk}"
    );
    assert!(
        !on_disk.contains(tmp.path().to_str().unwrap()),
        "in-workspace path should not include the absolute tmp prefix: {on_disk}"
    );
    // Out-of-workspace path kept absolute (fallback for shared caches / external archives).
    assert!(on_disk.contains("\"/global/cache/external.mem\""));

    // Re-load reconstructs absolute paths.
    let reloaded = store.load(tmp.path()).unwrap();
    match &reloaded.mounts[0].storage {
        MountStorage::GitBranch { gitdir, .. } => {
            assert_eq!(gitdir, &tmp.path().join("mem-repo").join(".git"));
        }
        other => panic!("expected GitBranch storage, got {other:?}"),
    }
    match &reloaded.mounts[1].storage {
        MountStorage::Archive { path } => {
            assert_eq!(path, &PathBuf::from("/global/cache/external.mem"));
        }
        other => panic!("expected Archive storage, got {other:?}"),
    }
}

#[test]
fn load_absolute_inside_root_path_then_save_rewrites_relative() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    // Hand-write a mounts.json carrying an absolute gitdir that
    // lives inside this workspace_root — simulates the "committed
    // by another operator's home dir" failure mode that motivated
    // relative serialisation.
    let mounts_path = FileWorkspaceStore::mounts_json_path(tmp.path());
    std::fs::create_dir_all(mounts_path.parent().unwrap()).unwrap();
    let abs_gitdir = tmp.path().join("mem-repo").join(".git");
    let mounts_body = format!(
        r#"{{
  "format": "memstead-mounts-3",
  "mounts": [
    {{
      "mem": "engine",
      "schema": "default@1.0.0",
      "storage": {{
        "type": "git-branch",
        "gitdir": "{}",
        "branch": "engine"
      }},
      "capability": "write",
      "lifecycle": "eager",
      "cross_linkable": true
    }}
  ]
}}"#,
        abs_gitdir.to_str().unwrap()
    );
    std::fs::write(&mounts_path, &mounts_body).unwrap();

    let store = FileWorkspaceStore::new();
    // The reader accepts the file; the absolute path round-trips
    // untouched (it's already absolute).
    let workspace = store.load(tmp.path()).unwrap();
    match &workspace.mounts[0].storage {
        MountStorage::GitBranch { gitdir, .. } => assert_eq!(gitdir, &abs_gitdir),
        other => panic!("expected GitBranch storage, got {other:?}"),
    }

    // Saving the same workspace rewrites the file with a relative
    // path — self-healing, no explicit command needed.
    store.save_state(tmp.path(), &workspace).unwrap();
    let on_disk = std::fs::read_to_string(&mounts_path).unwrap();
    assert!(on_disk.contains("\"memstead-mounts-3\""));
    assert!(on_disk.contains("\"mem-repo/.git\""));
    assert!(!on_disk.contains(tmp.path().to_str().unwrap()));
}

/// Item 03 round-trip: writing a `MountStorage::GitBranch` mount
/// whose `branch` already carries the canonical `refs/heads/<leaf>`
/// form serialises that exact string into `mounts.json` (no
/// rewrite, no truncation), and reading the file back produces a
/// `Mount` whose in-memory `branch` equals the input verbatim. The
/// write path is the one source of truth for the on-disk shape — a
/// regression that re-introduces short-form writes would surface
/// here as a mismatch against `refs/heads/demo/engine`.
#[test]
fn save_state_preserves_refs_heads_branch_form() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let original = Workspace {
        mounts: vec![Mount {
            mem: "engine".to_string(),
            schema: Some(pin("default@1.0.0")),
            storage: MountStorage::GitBranch {
                gitdir: tmp.path().join("mem-repo").join(".git"),
                branch: "refs/heads/demo/engine".to_string(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        }],
        settings: WorkspaceSettings::default(),
    };
    store.save_state(tmp.path(), &original).unwrap();

    let on_disk =
        std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
    assert!(
        on_disk.contains("\"branch\": \"refs/heads/demo/engine\""),
        "expected fully-qualified ref on disk, got: {on_disk}"
    );

    let reloaded = store.load(tmp.path()).unwrap();
    match &reloaded.mounts[0].storage {
        MountStorage::GitBranch { branch, .. } => {
            assert_eq!(branch, "refs/heads/demo/engine");
        }
        other => panic!("expected GitBranch storage, got {other:?}"),
    }
}

/// Item 03 reader tolerance: a hand-written legacy `mounts.json`
/// whose `branch` field carries the short-form leaf (no
/// `refs/heads/` prefix) loads without error, and the in-memory
/// `Mount` carries the input string intact. The reader does not
/// silently normalise — backend factories are responsible for
/// fully-qualifying short forms at instantiation time. This pin
/// guards against an over-eager normaliser landing on the read
/// path and masking out the legacy shape that older committed
/// `mounts.json` files used to carry.
#[test]
fn load_preserves_short_form_branch_without_rewrite() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let mounts_path = FileWorkspaceStore::mounts_json_path(tmp.path());
    std::fs::create_dir_all(mounts_path.parent().unwrap()).unwrap();
    std::fs::write(
        &mounts_path,
        r#"{
  "format": "memstead-mounts-3",
  "mounts": [
    {
      "mem": "engine",
      "schema": "default@1.0.0",
      "storage": {
        "type": "git-branch",
        "gitdir": "mem-repo/.git",
        "branch": "demo/engine"
      },
      "capability": "write",
      "lifecycle": "eager",
      "cross_linkable": true
    }
  ]
}"#,
    )
    .unwrap();

    let store = FileWorkspaceStore::new();
    let workspace = store.load(tmp.path()).unwrap();
    match &workspace.mounts[0].storage {
        MountStorage::GitBranch { branch, .. } => {
            assert_eq!(
                branch, "demo/engine",
                "reader must not silently rewrite short-form branch"
            );
        }
        other => panic!("expected GitBranch storage, got {other:?}"),
    }
}

#[test]
fn load_rejects_format_version_mismatch_on_toml() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-99"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let err = store.load(tmp.path()).unwrap_err();
    match err {
        StoreError::FormatMismatch {
            expected, found, ..
        } => {
            assert_eq!(expected, "memstead-git-branch-2");
            assert_eq!(found, "memstead-git-branch-99");
        }
        other => panic!("expected FormatMismatch, got {other:?}"),
    }
}

#[test]
fn load_rejects_format_version_mismatch_on_mounts_json() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let mounts_path = FileWorkspaceStore::mounts_json_path(tmp.path());
    std::fs::create_dir_all(mounts_path.parent().unwrap()).unwrap();
    std::fs::write(
        &mounts_path,
        r#"{ "format": "memstead-mounts-99", "mounts": [] }"#,
    )
    .unwrap();
    let store = FileWorkspaceStore::new();
    let err = store.load(tmp.path()).unwrap_err();
    assert!(matches!(err, StoreError::FormatMismatch { .. }));
}

/// A pre-rename workspace.toml (format V1) refuses with the typed
/// LegacyLayout error — it must not boot empty or half-parsed.
#[test]
fn load_refuses_pre_rename_toml_as_legacy_layout() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-1"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let err = store.load(tmp.path()).unwrap_err();
    match err {
        StoreError::LegacyLayout { found, .. } => {
            assert_eq!(found, "memstead-git-branch-1");
        }
        other => panic!("expected LegacyLayout, got {other:?}"),
    }
}

/// Pre-rename mounts.json formats refuse with LegacyLayout even
/// though their records no longer deserialise (old unit-noun
/// field name) — the format probe must win over the record-level
/// parse error so the agent sees the migration hint, not a serde
/// message. The fixture's record deliberately lacks the `mem`
/// field to prove the full parse is never reached.
#[test]
fn load_refuses_pre_rename_mounts_json_as_legacy_layout() {
    for legacy in ["memstead-mounts-1", "memstead-mounts-2"] {
        let tmp = TempDir::new().unwrap();
        write_workspace_toml(
            tmp.path(),
            r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
        );
        let mounts_path = FileWorkspaceStore::mounts_json_path(tmp.path());
        std::fs::create_dir_all(mounts_path.parent().unwrap()).unwrap();
        std::fs::write(
                &mounts_path,
                format!(
                    r#"{{ "format": "{legacy}", "mounts": [{{ "unit": "notes", "storage": {{ "type": "folder", "path": "notes" }}, "capability": "write", "lifecycle": "eager", "cross_linkable": true }}] }}"#
                ),
            )
            .unwrap();
        let store = FileWorkspaceStore::new();
        let err = store.load(tmp.path()).unwrap_err();
        match err {
            StoreError::LegacyLayout { found, .. } => assert_eq!(found, legacy),
            other => panic!("expected LegacyLayout for {legacy}, got {other:?}"),
        }
    }
}

#[test]
fn load_rejects_invalid_toml() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(tmp.path(), "this is not = valid = toml");
    let store = FileWorkspaceStore::new();
    let err = store.load(tmp.path()).unwrap_err();
    assert!(matches!(err, StoreError::Parse { .. }));
}

#[test]
fn load_rejects_unknown_top_level_key() {
    // The workspace config's contract (and its shipped example's
    // claim) is that typos never pass silently — at the top level,
    // not just inside [mcp]/[mutations].
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        "format = \"memstead-git-branch-2\"\nnonexistent_key = true\n",
    );
    let store = FileWorkspaceStore::new();
    let err = store.load(tmp.path()).unwrap_err();
    match err {
        StoreError::Parse { message, .. } => {
            assert!(
                message.contains("nonexistent_key"),
                "refusal must name the unknown key: {message}"
            );
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn instantiate_local_backend_handles_folder_archive_and_in_memory() {
    let tmp = TempDir::new().unwrap();
    let folder = folder_mount("local", tmp.path().to_path_buf());
    let archive_path = tmp.path().join("ext.mem");
    // Make a minimal valid zip so the archive backend can open it.
    let f = std::fs::File::create(&archive_path).unwrap();
    let mut w = zip::ZipWriter::new(f);
    w.start_file("a.md", zip::write::SimpleFileOptions::default())
        .unwrap();
    w.write_all(b"# a").unwrap();
    w.finish().unwrap();
    let archive = Mount {
        mem: "external".to_string(),
        schema: Some(pin("default@1.0.0")),
        storage: MountStorage::Archive { path: archive_path },
        capability: MountCapability::ReadOnly,
        lifecycle: MountLifecycle::Lazy,
        cross_linkable: false,
        migration_target: None,
    };
    let in_memory = Mount {
        mem: "session".to_string(),
        schema: Some(pin("default@1.0.0")),
        storage: MountStorage::InMemory,
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };

    let _: Box<dyn MemBackend> = instantiate_local_backend(&folder).unwrap();
    let _: Box<dyn MemBackend> = instantiate_local_backend(&archive).unwrap();
    // The in-memory variant is a local backend: no factory needed,
    // no path, materialises directly.
    let _: Box<dyn MemBackend> = instantiate_local_backend(&in_memory).unwrap();
}

/// AC3 (plan 01): the in-memory storage variant round-trips through
/// `mounts.json` and its wire shape is unambiguous — it serialises
/// as the bare `{"type":"in-memory"}` tag and never parses as one
/// of the path-carrying variants, nor they as it.
#[test]
fn save_state_round_trips_in_memory_variant_unambiguously() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        r#"
format = "memstead-git-branch-2"

[persistence_adapter]
name = "file-two-layer"
"#,
    );
    let store = FileWorkspaceStore::new();
    let original = Workspace {
        mounts: vec![
            folder_mount("local", PathBuf::from("/work/mem")),
            Mount {
                mem: "session".to_string(),
                schema: Some(pin("default@1.0.0")),
                storage: MountStorage::InMemory,
                capability: MountCapability::Write,
                lifecycle: MountLifecycle::Eager,
                cross_linkable: true,
                migration_target: None,
            },
        ],
        settings: WorkspaceSettings::default(),
    };
    store.save_state(tmp.path(), &original).unwrap();

    // On the wire it is the bare tag — no `path`, no `gitdir`.
    let raw = std::fs::read_to_string(FileWorkspaceStore::mounts_json_path(tmp.path())).unwrap();
    assert!(raw.contains("\"type\": \"in-memory\""), "got: {raw}");

    let reloaded = store.load(tmp.path()).unwrap();
    assert_eq!(reloaded.mounts.len(), 2);
    // The in-memory mount round-trips back to exactly InMemory —
    // not silently reinterpreted as a folder/archive/git variant.
    let session = reloaded
        .mounts
        .iter()
        .find(|m| m.mem == "session")
        .expect("session mount survives reload");
    assert_eq!(session.storage, MountStorage::InMemory);
    // And the sibling folder mount is untouched — the two wire
    // shapes do not bleed into each other.
    let local = reloaded.mounts.iter().find(|m| m.mem == "local").unwrap();
    assert!(matches!(local.storage, MountStorage::Folder { .. }));
}

#[test]
fn instantiate_local_backend_rejects_git_branch_with_typed_error() {
    let mount = Mount {
        mem: "engine".to_string(),
        schema: Some(pin("default@1.0.0")),
        storage: MountStorage::GitBranch {
            gitdir: PathBuf::from("/some/path/.git"),
            branch: "engine".to_string(),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    // `unwrap_err()` requires Box<dyn MemBackend> to be Debug;
    // matching on the Result keeps the test gix-free of that
    // bound while still asserting the typed error.
    match instantiate_local_backend(&mount) {
        Err(InstantiateError::GitBranchBackendUnavailable { mem }) => {
            assert_eq!(mem, "engine");
        }
        Ok(_) => panic!("expected GitBranchBackendUnavailable, got Ok"),
    }
}

#[test]
fn detect_layout_returns_empty_for_unrecognised_workspace() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(detect_layout(tmp.path()), Layout::Empty);
}
#[test]
fn detect_layout_returns_new_when_workspace_toml_present() {
    let tmp = TempDir::new().unwrap();
    write_workspace_toml(
        tmp.path(),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );
    assert_eq!(detect_layout(tmp.path()), Layout::New);
}
