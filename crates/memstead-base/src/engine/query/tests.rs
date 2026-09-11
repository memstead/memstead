#![cfg(test)]

use std::path::Path;

use tempfile::TempDir;

use crate::backend::{BackendError, MemBackend};
use crate::engine::test_helpers::*;
use crate::engine::{Engine, EngineError, RelateEntityArgs};
use crate::entity::EntityId;
use crate::ops::{Direction, SearchScope, WarningHint};
use crate::provenance::Provenance;
use crate::storage::{ArchiveBackend, FilesystemBackend};

use crate::vcs::CommitContext;
use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

/// The shared decision-29 candidate rule: source-join first,
/// workspace-relative fallback; a pointer-less (or `.`) source has one
/// reading; a climbing `../…` artifact never joins (the fabricated
/// `<ptr>/../…` would resolve into a sibling tree).
#[test]
fn artifact_candidates_follow_decision_29_priority() {
    use super::artifact_candidates;
    assert_eq!(artifact_candidates("", "a/b.rs"), vec!["a/b.rs"]);
    assert_eq!(artifact_candidates(".", "a/b.rs"), vec!["a/b.rs"]);
    assert_eq!(artifact_candidates("./", "a/b.rs"), vec!["a/b.rs"]);
    assert_eq!(
        artifact_candidates("sub", "a/b.rs"),
        vec!["sub/a/b.rs", "a/b.rs"]
    );
    assert_eq!(
        artifact_candidates("sub/", "a/b.rs"),
        vec!["sub/a/b.rs", "a/b.rs"]
    );
    // Self-nesting is settled by priority, not suppression: the artifact
    // already carrying the pointer prefix still offers the join first.
    assert_eq!(
        artifact_candidates("sub", "sub/x.rs"),
        vec!["sub/sub/x.rs", "sub/x.rs"]
    );
    // A climbing artifact is already the workspace-relative form.
    assert_eq!(
        artifact_candidates("sub", "../dev/x.md"),
        vec!["../dev/x.md"]
    );
    assert_eq!(
        artifact_candidates("../dev", "../dev/x.md"),
        vec!["../dev/x.md"]
    );
    assert_eq!(artifact_candidates("sub", ".."), vec![".."]);
}

/// `schema_origin` is the trust-classification authority: a built-in
/// (or workspace-authored) schema is first-party; a schema whose
/// `(name, version)` is in neither catalogue is third-party — the safe
/// default for an origin the engine cannot vouch for.
#[test]
fn schema_origin_classifies_builtin_first_party_and_unknown_third_party() {
    use std::sync::Arc;

    use crate::render::OriginClass;

    let tmp = TempDir::new().unwrap();
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", tmp.path().to_path_buf()),
        Box::new(FilesystemBackend::new(tmp.path().to_path_buf())) as Box<dyn MemBackend>,
    )])
    .unwrap();

    // A built-in schema (the catalogue the engine resolved against).
    let builtin = engine.builtin_schemas()[0].clone();
    assert_eq!(
        engine.schema_origin(&builtin),
        OriginClass::FirstParty,
        "a built-in schema is first-party"
    );

    // A schema whose version is in no catalogue — a stand-in for a
    // schema that entered from outside the workspace. Same name, a
    // version the engine never loaded.
    let foreign = Arc::new(memstead_schema::Schema {
        manifest: builtin.manifest.clone(),
        version: semver::Version::new(99, 0, 0),
        types: builtin.types.clone(),
    });
    assert_eq!(
        engine.schema_origin(&foreign),
        OriginClass::ThirdParty,
        "a schema in neither catalogue classifies third-party (safe default)"
    );
}

/// `mem_origin_class` classifies a writable mount first-party (its
/// content is authored in this workspace) and a read-only mount
/// third-party (registry-installed read-mem or adopted foreign
/// folder/clone — quoted, untrusted data). An unknown mem is
/// third-party (the safe default).
#[test]
fn mem_origin_class_writable_first_party_readonly_third_party() {
    use crate::render::OriginClass;

    let tmp = TempDir::new().unwrap();
    // Writable folder mem.
    let writable_dir = tmp.path().join("writable");
    std::fs::create_dir_all(&writable_dir).unwrap();
    let writer = FilesystemBackend::new(writable_dir.clone());

    // Read-only archive mem.
    let body = "---\ntype: spec\n---\n# Ext\n\n## Identity\n\nFrom an archive.\n";
    let archive_path = build_archive(tmp.path(), "ext", &[("ext.md", body.as_bytes())]);

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("local", writable_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("external", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    assert_eq!(
        engine.mem_origin_class("local"),
        OriginClass::FirstParty,
        "a writable mount is first-party"
    );
    assert_eq!(
        engine.mem_origin_class("external"),
        OriginClass::ThirdParty,
        "a read-only mount is third-party"
    );
    assert_eq!(
        engine.mem_origin_class("no-such-mem"),
        OriginClass::ThirdParty,
        "an unknown mem is third-party (safe default)"
    );
}

/// `declare_mem_origin` lets the embedding deployment vouch for one
/// read-only mount as first-party (the curated hosted read tier),
/// overriding the writability inference for that mem only — sibling
/// read-only mounts keep the safe third-party default.
#[test]
fn declared_origin_overrides_inference_per_mem() {
    use crate::render::OriginClass;

    let tmp = TempDir::new().unwrap();
    let body = "---\ntype: spec\n---\n# Ext\n\n## Identity\n\nFrom an archive.\n";
    let vouched_path = build_archive(tmp.path(), "vouched", &[("v.md", body.as_bytes())]);
    let other_path = build_archive(tmp.path(), "other", &[("o.md", body.as_bytes())]);

    let mut engine = Engine::from_mounts(vec![
        (
            archive_mount("vouched", vouched_path.clone()),
            Box::new(ArchiveBackend::new(vouched_path)) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("other", other_path.clone()),
            Box::new(ArchiveBackend::new(other_path)) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    engine.declare_mem_origin("vouched", OriginClass::FirstParty);

    assert_eq!(
        engine.mem_origin_class("vouched"),
        OriginClass::FirstParty,
        "the deployment's declaration wins over the read-only inference"
    );
    assert_eq!(
        engine.mem_origin_class("other"),
        OriginClass::ThirdParty,
        "an undeclared sibling mount keeps the safe default"
    );
}

/// The adopt-gate: a non-built-in schema is first-party only once a
/// writable mount pins it (the operator authors against it here).
/// Pinned only by a read-only mount — a registry read-mem or an
/// adopted foreign folder/clone — it stays third-party, so
/// `memstead_schema` serves it structural-only.
#[test]
fn schema_origin_third_party_until_pinned_by_a_writable_mount() {
    use memstead_schema::SchemaRef;

    use crate::render::OriginClass;

    let manifest = r#"name: trust-test
version: 0.1.0
description: adopt-gate test schema
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
    let pin = SchemaRef::new("trust-test", semver::Version::new(0, 1, 0));

    let mk_engine = |cap: MountCapability| -> Engine {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        write_schema_files_with_default_type(&schemas_dir, "trust-test", manifest, &["doc"]);
        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let mount = Mount {
            mem: "v".to_string(),
            schema: Some(pin.clone()),
            storage: MountStorage::Folder {
                path: mem_dir.clone(),
            },
            capability: cap,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend = Box::new(FilesystemBackend::new(mem_dir)) as Box<dyn MemBackend>;
        // Keep `tmp` alive for the engine's lifetime by leaking it —
        // the test process is short-lived and the folder must outlast
        // the closure.
        std::mem::forget(tmp);
        Engine::from_mounts_with_schemas_dir(vec![(mount, backend)], Some(&schemas_dir)).unwrap()
    };

    // Read-only mount: the foreign schema is never adopted → third-party.
    let ro = mk_engine(MountCapability::ReadOnly);
    let schema = ro.schemas().get("v").expect("schema resolved").clone();
    assert_eq!(
        ro.schema_origin(&schema),
        OriginClass::ThirdParty,
        "a non-built-in schema pinned only by a read-only mount is third-party"
    );

    // Writable mount pinning the same schema: adopted → first-party.
    let rw = mk_engine(MountCapability::Write);
    let schema = rw.schemas().get("v").expect("schema resolved").clone();
    assert_eq!(
        rw.schema_origin(&schema),
        OriginClass::FirstParty,
        "a writable mount pinning the schema adopts it → first-party"
    );
}

/// Consumer read path: an installed (archive-backed) mem that ships
/// a `.memstead/provenance.json` payload surfaces per-entity authoring
/// provenance through `archive_provenance_for`. A noted entity carries
/// its rationale; an entity authored without a note is absent from the
/// payload and reads as provenance-absent (no fabricated value); the
/// `history` disposition records that full history is not shipped.
#[test]
fn archive_provenance_surfaces_per_entity_and_reports_absence() {
    use memstead_schema::History;

    let tmp = TempDir::new().unwrap();
    let config = br#"{"format":3,"name":"seed","version":"0.1.0","schema":"default@1.0.0"}"#;
    let alpha = b"---\ntype: spec\n---\n# Alpha\n\n## Identity\n\na\n\n## Purpose\n\np\n";
    let beta = b"---\ntype: spec\n---\n# Beta\n\n## Identity\n\nb\n\n## Purpose\n\np\n";
    // alpha noted; beta deliberately absent from the payload.
    let provenance = br#"{"format":1,"history":"summarised","entities":{"alpha":{"rationale":"why alpha exists","kind":"create","timestamp":"2026-06-24T00:00:00Z","actor":"agent"}}}"#;
    let archive = build_archive(
        tmp.path(),
        "seed",
        &[
            (".memstead/config.json", config),
            ("alpha.md", alpha),
            ("beta.md", beta),
            (".memstead/provenance.json", provenance),
        ],
    );
    let engine = Engine::from_mounts(vec![(
        archive_mount("seed", archive.clone()),
        Box::new(ArchiveBackend::new(archive)) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let prov = engine
        .archive_provenance_for("seed")
        .expect("provenance payload read from the archive");
    assert_eq!(
        prov.history,
        History::Summarised,
        "history-not-shipped is observable"
    );
    assert_eq!(
        prov.entity("alpha").and_then(|r| r.rationale.as_deref()),
        Some("why alpha exists"),
        "noted entity surfaces its rationale"
    );
    assert!(
        prov.entity("beta").is_none(),
        "unnoted entity is absent (reported absent, not fabricated)"
    );
}

/// A pre-provenance archive (no `.memstead/provenance.json`) reads as
/// provenance uniformly absent — the additive contract: a newer engine
/// installing an old archive reports no provenance, never an error.
#[test]
fn archive_without_provenance_reports_absent() {
    let tmp = TempDir::new().unwrap();
    let config = br#"{"format":3,"name":"seed","version":"0.1.0","schema":"default@1.0.0"}"#;
    let alpha = b"---\ntype: spec\n---\n# Alpha\n\n## Identity\n\na\n\n## Purpose\n\np\n";
    let archive = build_archive(
        tmp.path(),
        "seed",
        &[(".memstead/config.json", config), ("alpha.md", alpha)],
    );
    let engine = Engine::from_mounts(vec![(
        archive_mount("seed", archive.clone()),
        Box::new(ArchiveBackend::new(archive)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(
        engine.archive_provenance_for("seed").is_none(),
        "an archive without a provenance payload reports provenance absent"
    );
}

#[test]
fn folder_mount_routes_reads_to_filesystem_backend() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    // Seeding goes through the MemBackend trait; the
    // module-top `use` brings both into scope. Seed via fully-
    // qualified MemBackend calls so dot-syntax stays unambiguous.
    <FilesystemBackend as MemBackend>::write_entity(&writer, Path::new("a.md"), b"alpha").unwrap();
    <FilesystemBackend as MemBackend>::commit(&writer, "seed", &CommitContext::internal()).unwrap();

    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let mut paths: Vec<String> = engine
        .list_entities("specs")
        .unwrap()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["a.md".to_string()]);

    assert_eq!(
        engine.read_entity("specs", Path::new("a.md")).unwrap(),
        Some(b"alpha".to_vec())
    );
}

#[test]
fn heterogeneous_mounts_route_to_correct_backend() {
    let tmp = TempDir::new().unwrap();

    // Folder mem.
    let folder_dir = tmp.path().join("folder-mem");
    std::fs::create_dir_all(&folder_dir).unwrap();
    let folder_writer = FilesystemBackend::new(folder_dir.clone());
    <FilesystemBackend as MemBackend>::write_entity(
        &folder_writer,
        Path::new("local.md"),
        b"local",
    )
    .unwrap();
    <FilesystemBackend as MemBackend>::commit(&folder_writer, "seed", &CommitContext::internal())
        .unwrap();

    // Archive mem.
    let archive_path = build_archive(
        tmp.path(),
        "external",
        &[("ext.md", b"external"), ("dir/nested.md", b"nested")],
    );

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("local", folder_dir),
            Box::new(folder_writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("external", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        ),
    ])
    .unwrap();

    // Routes correctly by mem name.
    assert_eq!(engine.mem_names(), vec!["local", "external"]);
    assert_eq!(
        engine.read_entity("local", Path::new("local.md")).unwrap(),
        Some(b"local".to_vec())
    );
    assert_eq!(
        engine.read_entity("external", Path::new("ext.md")).unwrap(),
        Some(b"external".to_vec())
    );
    assert_eq!(
        engine
            .read_entity("external", Path::new("dir/nested.md"))
            .unwrap(),
        Some(b"nested".to_vec())
    );
    // Cross-routing: reading a path from the wrong mem → None
    // (the backend doesn't have it), not an error.
    assert_eq!(
        engine.read_entity("local", Path::new("ext.md")).unwrap(),
        None
    );
    assert_eq!(
        engine
            .read_entity("external", Path::new("local.md"))
            .unwrap(),
        None
    );
}

#[test]
fn edge_is_from_readonly_classifies_every_edge_by_source_mount_capability() {
    // `engine.edge_is_from_readonly` is the derived-on-demand
    // alternative to adding a per-edge marker: construct a mixed
    // workspace (one Write-Mem + one ReadOnly archive with
    // cross-mem wiki-links) and walk every edge in the store,
    // asserting each edge's source-mount capability.
    let tmp = TempDir::new().unwrap();

    // Write folder mem `local` with a spec-shaped entity that
    // declares an explicit cross-mem relation into the archive
    // (under the alias model edges originate from `## Relationships`).
    let folder_dir = tmp.path().join("local-mem");
    std::fs::create_dir_all(&folder_dir).unwrap();
    let folder_writer = FilesystemBackend::new(folder_dir.clone());
    let local_md = b"---\ntype: spec\n---\n# Note\n\n## Identity\n\nsee [[external:archived]] for prior context.\n\n## Relationships\n\n- **REFERENCES**: [[external:archived]]\n";
    <FilesystemBackend as MemBackend>::write_entity(&folder_writer, Path::new("note.md"), local_md)
        .unwrap();
    <FilesystemBackend as MemBackend>::commit(&folder_writer, "seed", &CommitContext::internal())
        .unwrap();

    // ReadOnly archive mem `external` with a spec-shaped entity
    // declaring an explicit cross-mem relation back to the local
    // note.
    let archive_md = b"---\ntype: spec\n---\n# Archived\n\n## Identity\n\nrefers back to [[local:note]] for the current revision.\n\n## Relationships\n\n- **REFERENCES**: [[local:note]]\n";
    let archive_path = build_archive(tmp.path(), "external", &[("archived.md", archive_md)]);

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("local", folder_dir),
            Box::new(folder_writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("external", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        ),
    ])
    .unwrap();

    // Sanity: both entities are real, both mems are mounted.
    let local_id = EntityId::new("local", "note");
    let archived_id = EntityId::new("external", "archived");
    assert!(engine.get_entity(&local_id).is_some());
    assert!(engine.get_entity(&archived_id).is_some());
    assert!(matches!(
        engine.capability("local").unwrap(),
        MountCapability::Write
    ));
    assert!(matches!(
        engine.capability("external").unwrap(),
        MountCapability::ReadOnly
    ));

    // Walk every edge in the store. For each (from, edge) pair,
    // `edge_is_from_readonly(from)` must return true iff the
    // source mount's capability is ReadOnly. The fixture's two
    // wiki-links produce one edge from each mem — both halves
    // exercise both branches of the helper.
    let mut seen_write_edge = false;
    let mut seen_readonly_edge = false;
    for from in engine.store().all_ids().cloned().collect::<Vec<_>>() {
        for _edge in engine.store().outgoing(&from) {
            let is_ro = engine.edge_is_from_readonly(&from);
            match engine.capability(from.mem()).unwrap() {
                MountCapability::Write => {
                    assert!(
                        !is_ro,
                        "edge from write mem {} reported as ReadOnly",
                        from.mem()
                    );
                    seen_write_edge = true;
                }
                MountCapability::ReadOnly => {
                    assert!(
                        is_ro,
                        "edge from readonly mem {} reported as Write",
                        from.mem()
                    );
                    seen_readonly_edge = true;
                }
            }
        }
    }
    assert!(
        seen_write_edge,
        "fixture must produce at least one edge from a write mem"
    );
    assert!(
        seen_readonly_edge,
        "fixture must produce at least one edge from a readonly mem"
    );

    // Helper also reports `false` for mems absent from the
    // router — no mount → no ReadOnly assertion can be made.
    let phantom = EntityId::new("missing-mem", "phantom");
    assert!(
        !engine.edge_is_from_readonly(&phantom),
        "absent mount must not be reported as ReadOnly"
    );
}

// ---- Engine::changes_since wrapper ------------------------------

#[test]
fn cross_mem_link_allowed_same_mem_always_true() {
    // Self-edges (from == to) bypass the cross-mem policy
    // entirely — the policy gates *cross*-mem edges only.
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    assert!(engine.cross_mem_link_allowed("specs", "specs"));
    // Even when the mem doesn't exist (not enrolled in
    // settings.cross_mem_links), same-mem returns true —
    // the engine doesn't validate mem existence here, just the
    // policy.
    assert!(engine.cross_mem_link_allowed("anywhere", "anywhere"));
}

#[test]
fn cross_mem_link_allowed_absent_denies_by_default() {
    // No entry in cross_mem_links for `from_mem` → denied.
    // Default-deny is the V1 posture; operators opt in.
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    assert!(!engine.cross_mem_link_allowed("specs", "engine"));
    assert!(!engine.cross_mem_link_allowed("missing", "anywhere"));
}

#[test]
fn cross_mem_link_allowed_wildcard_admits_any_target() {
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings
        .cross_mem_links
        .insert("specs".to_string(), CrossLinkValue::Wildcard);
    engine.set_settings(settings);
    assert!(engine.cross_mem_link_allowed("specs", "engine"));
    assert!(engine.cross_mem_link_allowed("specs", "macos"));
    assert!(engine.cross_mem_link_allowed("specs", "any-other"));
    // Reverse direction is independent — no policy entry for
    // engine→specs means denied.
    assert!(!engine.cross_mem_link_allowed("engine", "specs"));
}

#[test]
fn cross_mem_link_allowed_allowlist_enforces_membership() {
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "specs".to_string(),
        CrossLinkValue::List(vec!["engine".to_string(), "macos".to_string()]),
    );
    engine.set_settings(settings);
    assert!(engine.cross_mem_link_allowed("specs", "engine"));
    assert!(engine.cross_mem_link_allowed("specs", "macos"));
    assert!(!engine.cross_mem_link_allowed("specs", "external"));
}

#[test]
fn cross_mem_link_allowed_synthesises_from_matching_create_rule_wildcard() {
    // No explicit cross_mem_links entry, but a create rule
    // matches `from_mem` and carries default_cross_links = "*".
    // Synthesis grants permission to any target.
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings
        .mem_create_rules
        .push(crate::workspace::CreateRuleSetting {
            pattern: "exec-*".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::Wildcard),
        });
    engine.set_settings(settings);
    // No explicit policy; synthesis grants permission for any
    // target because the rule's value is Wildcard.
    assert!(engine.cross_mem_link_allowed("exec-foo", "specs"));
    assert!(engine.cross_mem_link_allowed("exec-foo", "engine"));
    // Mem that doesn't match any rule → still denied.
    assert!(!engine.cross_mem_link_allowed("orphan", "specs"));
}

/// #42: synthesis matches a hierarchical mem by composing the same
/// `<mem_path>/<name>` candidate the create-rule glob is keyed on,
/// not the bare leaf. Before the fix, `from_mem = "project"` could
/// never match a `memstead/*` rule (the leaf-vs-composed-path
/// divergence), so enforcement denied a link `memstead_overview`
/// rendered as rule-granted.
#[test]
fn cross_mem_link_allowed_synthesises_for_hierarchical_mem() {
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    // Mount `project` with a hierarchical branch so its `mem_path()`
    // is "memstead" and the composed candidate is "memstead/project".
    // The Folder backend handles loading; only the Mount's storage
    // feeds `mem_path()`.
    let mount = Mount {
        mem: "project".into(),
        schema: Some(pin("default")),
        storage: MountStorage::GitBranch {
            gitdir: mem_dir.join(".git"),
            branch: "memstead/project".into(),
        },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine =
        Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings
        .mem_create_rules
        .push(crate::workspace::CreateRuleSetting {
            pattern: "memstead/*".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::List(vec!["engine".to_string()])),
        });
    engine.set_settings(settings);
    assert!(
        engine.cross_mem_link_allowed("project", "engine"),
        "synthesis must match via the composed `memstead/project` candidate"
    );
    assert!(
        !engine.cross_mem_link_allowed("project", "macos"),
        "a target outside the rule's default_cross_links is still denied"
    );
}

#[test]
fn cross_mem_link_allowed_synthesises_from_matching_create_rule_list() {
    // Create rule's default_cross_links is a list — synthesis
    // grants permission to listed targets only.
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings
        .mem_create_rules
        .push(crate::workspace::CreateRuleSetting {
            pattern: "exec-*".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::List(vec!["specs".to_string()])),
        });
    engine.set_settings(settings);
    assert!(engine.cross_mem_link_allowed("exec-foo", "specs"));
    // Target not in the synthesised list → denied.
    assert!(!engine.cross_mem_link_allowed("exec-foo", "engine"));
}

#[test]
fn cross_mem_link_allowed_explicit_policy_wins_over_synthesis() {
    // Explicit cross_mem_links wildcard fires first; the
    // synthesis layer is never consulted (and would deny).
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings
        .cross_mem_links
        .insert("exec-foo".to_string(), CrossLinkValue::Wildcard);
    // The synthesis layer would deny `exec-foo → engine` (no
    // matching rule), but explicit policy returns true first.
    engine.set_settings(settings);
    assert!(engine.cross_mem_link_allowed("exec-foo", "engine"));
}

#[test]
fn cross_mem_link_allowed_synthesis_unions_into_explicit_list() {
    // Explicit list = ["specs"]; create rule synthesises = ["macos"].
    // Effective allowed targets: union ({specs, macos}).
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings.cross_mem_links.insert(
        "exec-foo".to_string(),
        CrossLinkValue::List(vec!["specs".to_string()]),
    );
    settings
        .mem_create_rules
        .push(crate::workspace::CreateRuleSetting {
            pattern: "exec-*".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::List(vec!["macos".to_string()])),
        });
    engine.set_settings(settings);
    // Explicit allowlist contains specs → allowed.
    assert!(engine.cross_mem_link_allowed("exec-foo", "specs"));
    // Synthesis layer adds macos → allowed.
    assert!(engine.cross_mem_link_allowed("exec-foo", "macos"));
    // Neither layer allows engine → denied.
    assert!(!engine.cross_mem_link_allowed("exec-foo", "engine"));
}

#[test]
fn cross_mem_link_allowed_set_settings_invalidates_compiled_rule_cache() {
    // After set_settings, a fresh policy must be reflected on the
    // next call — the lazy memo can't return stale rules.
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);

    // First settings: a rule allows exec-* → specs via synthesis.
    let mut s1 = crate::workspace::WorkspaceSettings::default();
    s1.mem_create_rules
        .push(crate::workspace::CreateRuleSetting {
            pattern: "exec-*".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::List(vec!["specs".to_string()])),
        });
    engine.set_settings(s1);
    assert!(engine.cross_mem_link_allowed("exec-foo", "specs"));

    // Replace settings: the rule no longer carries
    // default_cross_links. Cache must invalidate so the next
    // call sees the new policy.
    let mut s2 = crate::workspace::WorkspaceSettings::default();
    s2.mem_create_rules
        .push(crate::workspace::CreateRuleSetting {
            pattern: "exec-*".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: None,
        });
    engine.set_settings(s2);
    assert!(!engine.cross_mem_link_allowed("exec-foo", "specs"));
}

#[test]
fn cross_mem_link_allowed_malformed_glob_falls_back_to_explicit_policy() {
    // Malformed pattern in a create rule causes CreateRuleSet
    // compilation to fail; the resolver logs and disables
    // synthesis, but explicit cross_mem_links still works.
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings
        .mem_create_rules
        .push(crate::workspace::CreateRuleSetting {
            pattern: "[unclosed".to_string(),
            schemas: vec!["default".to_string()],
            default_cross_links: Some(CrossLinkValue::Wildcard),
        });
    // Explicit policy still works.
    settings
        .cross_mem_links
        .insert("specs".to_string(), CrossLinkValue::Wildcard);
    engine.set_settings(settings);
    // Explicit policy: specs → engine allowed.
    assert!(engine.cross_mem_link_allowed("specs", "engine"));
    // Synthesis disabled (compilation failed); rule's would-be
    // wildcard doesn't apply.
    assert!(!engine.cross_mem_link_allowed("orphan", "anything"));
}

#[test]
fn cross_mem_link_allowed_empty_list_denies_all_cross_mem_targets() {
    // [cross_mem_links] specs = [] is the explicit
    // "intentionally locked down" shape — same effect as
    // default-deny but operator-acknowledged.
    use memstead_schema::workspace_config::CrossLinkValue;
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    let mut settings = crate::workspace::WorkspaceSettings::default();
    settings
        .cross_mem_links
        .insert("specs".to_string(), CrossLinkValue::List(Vec::new()));
    engine.set_settings(settings);
    // Same-mem still passes — policy only gates cross-mem.
    assert!(engine.cross_mem_link_allowed("specs", "specs"));
    // Cross-mem denied to every target.
    assert!(!engine.cross_mem_link_allowed("specs", "engine"));
    assert!(!engine.cross_mem_link_allowed("specs", "anything"));
}

#[test]
fn from_mounts_load_warnings_merge_into_health_summary() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let body = "---\ntype: spec\n---\n# Dup2\n\n## Identity\n\na.\n\n## Identity\n\nb.\n";
    std::fs::write(mem_dir.join("dup2.md"), body).unwrap();

    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let summary = engine.health();
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::DuplicateSectionHeading { .. })),
        "health() must merge load_warnings into summary.warnings: {:?}",
        summary.warnings,
    );
}

#[test]
fn workspace_root_accessor_is_none_for_engine_built_from_mounts() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    // Newest default generation so the clean-boot assertion below
    // isn't tripped by the SCHEMA_GENERATIONS_BEHIND hint.
    let mut mount = folder_mount("specs", mem_dir);
    mount.schema = Some("default@1.3.0".parse().unwrap());
    let engine =
        Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
    assert!(
        engine.workspace_root().is_none(),
        "from_mounts has no workspace path",
    );
    // An entity-less mount reports itself empty; nothing else.
    assert!(
        engine
            .load_warnings()
            .iter()
            .all(|w| w.code() == "MOUNT_UNBACKED"),
        "{:?}",
        engine.load_warnings()
    );
}

#[test]
fn health_omits_outer_repo_warning_when_workspace_root_unset() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let health = engine.health();
    assert!(
        !health
            .warnings
            .iter()
            .any(|w| matches!(w, WarningHint::OuterRepoNotIgnoringMemRepo { .. })),
        "outer-repo check must skip when workspace_root is None",
    );
}

#[test]
fn writable_mem_names_filters_by_capability() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("writable", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("sealed", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        ),
    ])
    .unwrap();

    // Only the writable mount surfaces; the archive (read-only)
    // is filtered out.
    let names = engine.writable_mem_names();
    assert_eq!(names, vec!["writable"]);
}

/// The default writable mem is
/// the FIRST writable mount in declaration order — the stable seed,
/// not the alphabetically-first name. `test` is declared first;
/// `other` sorts ahead alphabetically but is declared second, so it
/// is NOT the default. This is the invariant that stops a second
/// mem from silently retargeting omitted-`mem` writes.
#[test]
fn default_writable_mem_is_declaration_first_not_alphabetical() {
    let tmp = TempDir::new().unwrap();
    let test_dir = tmp.path().join("test");
    let other_dir = tmp.path().join("other");
    std::fs::create_dir_all(&test_dir).unwrap();
    std::fs::create_dir_all(&other_dir).unwrap();

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("test", test_dir.clone()),
            Box::new(FilesystemBackend::new(test_dir)) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("other", other_dir.clone()),
            Box::new(FilesystemBackend::new(other_dir)) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    assert_eq!(
        engine.default_writable_mem(),
        Some("test"),
        "default must be the declaration-first writable mem, not the alphabetically-first",
    );
}

/// Reverse declaration order to prove the default tracks declaration
/// order rather than a fixed name: with `other` declared first it
/// becomes the default. Together with the test above this pins the
/// lean as mount order, not name sort.
#[test]
fn default_writable_mem_follows_declaration_order() {
    let tmp = TempDir::new().unwrap();
    let other_dir = tmp.path().join("other");
    let test_dir = tmp.path().join("test");
    std::fs::create_dir_all(&other_dir).unwrap();
    std::fs::create_dir_all(&test_dir).unwrap();

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("other", other_dir.clone()),
            Box::new(FilesystemBackend::new(other_dir)) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("test", test_dir.clone()),
            Box::new(FilesystemBackend::new(test_dir)) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    assert_eq!(engine.default_writable_mem(), Some("other"));
}

/// A read-only-only workspace has no default writable mem.
#[test]
fn default_writable_mem_none_without_writable_mount() {
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
    let engine = Engine::from_mounts(vec![(
        archive_mount("sealed", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert_eq!(engine.default_writable_mem(), None);
}

#[test]
fn folder_path_for_mem_returns_path_for_folder_mounts_only() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("sealed", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        ),
    ])
    .unwrap();

    // Folder mount returns its path.
    assert_eq!(engine.folder_path_for_mem("specs"), Some(mem_dir.as_path()),);
    // Archive mount returns None — caller branches on storage type.
    assert_eq!(engine.folder_path_for_mem("sealed"), None);
    // Unknown mem returns None — same as Engine::mount.
    assert_eq!(engine.folder_path_for_mem("missing"), None);
}

#[test]
fn mount_accessor_returns_public_mount_shape() {
    // Build a heterogeneous engine and verify Engine::mount /
    // Engine::mounts surface the operator-facing Mount records.
    // Handlers branch on MountStorage variants through this
    // accessor (replacing full's gitdir_for / worktree_for /
    // mem_head_sha / mem_config_for direct-engine
    // accessors).
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("writable", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("sealed", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path.clone())),
        ),
    ])
    .unwrap();

    // Known mems: each returns a Mount whose storage variant
    // matches what the caller passed at construction.
    let folder = engine.mount("writable").expect("known mem");
    assert!(matches!(folder.storage, MountStorage::Folder { .. }));
    assert_eq!(folder.capability, MountCapability::Write);

    let archive = engine.mount("sealed").expect("known mem");
    match &archive.storage {
        MountStorage::Archive { path } => assert_eq!(path, &archive_path),
        other => panic!("expected Archive storage, got {other:?}"),
    }
    assert_eq!(archive.capability, MountCapability::ReadOnly);

    // Unknown mem — None, no panic, no error.
    assert!(engine.mount("missing").is_none());

    // Engine::mounts enumerates every mount in declaration order.
    let mounts = engine.mounts();
    assert_eq!(mounts.len(), 2);
    assert_eq!(mounts[0].mem, "writable");
    assert_eq!(mounts[1].mem, "sealed");
}

#[test]
fn mem_router_writable_set_matches_writable_mount_capability() {
    // Build an engine with one writable folder mount and one
    // read-only archive mount; the router's writable set must
    // equal the writable mount's name only.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("ext", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        ),
    ])
    .unwrap();

    let router = engine.mem_router();
    assert!(router.is_writable("specs"));
    assert!(!router.is_writable("ext"));
    assert!(router.is_visible("specs"));
    assert!(router.is_visible("ext"));
    let writable: std::collections::HashSet<&String> = router.writable_mems().iter().collect();
    assert_eq!(writable.len(), 1);
    assert!(writable.contains(&"specs".to_string()));
}

#[test]
fn mem_router_origin_is_explicit_toml_for_workspace_mounts() {
    // Every mount built via `from_mounts` lands as
    // `MemOrigin::ExplicitToml` — the file-adapter origin.
    // `RuntimeCreated` is reserved for `memstead_mem_create`
    // runtime registrations once that handler migrates onto
    // the unified engine.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());

    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let origin = engine
        .mem_router()
        .origin_for_mem("specs")
        .expect("known mem");
    assert_eq!(origin.kind(), "explicit");
}

#[test]
fn mem_router_dir_for_writable_folder_mount_matches_storage_path() {
    // Folder-backed writable mounts surface the storage path
    // via `dir_for_mem`. Handlers consuming the router for
    // per-mem path resolution rely on this.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());

    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    assert_eq!(
        engine.mem_router().dir_for_mem("specs"),
        Some(mem_dir.as_path()),
    );
    assert_eq!(engine.mem_router().dir_for_mem("unknown"), None);
}

#[test]
fn mem_router_archive_path_for_read_only_archive_mount() {
    // Read-only archive mounts register via `add_read_only` so
    // `archive_path_for_mem` resolves the archive's on-disk
    // location.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(&mem_dir).unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("ext", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path.clone())),
        ),
    ])
    .unwrap();

    let router = engine.mem_router();
    assert_eq!(
        router.archive_path_for_mem("ext"),
        Some(archive_path.as_path()),
    );
    // Writable folder mount has no archive path.
    assert_eq!(router.archive_path_for_mem("specs"), None);
}

#[test]
fn read_mem_config_via_backend_trait_folder_reads_bytes() {
    // Direct trait call against FilesystemBackend. Verifies
    // the backend-side primitive returns the raw bytes the
    // engine then parses.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    let body = br#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": { "tone": "neutral" }
        }"#;
    std::fs::write(mem_dir.join(".memstead").join("config.json"), body).unwrap();

    let writer = FilesystemBackend::new(mem_dir);
    let result = MemBackend::read_mem_config(&writer).unwrap();
    let bytes = result.expect("config bytes must surface");
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["schema"], "default@1.0.0");
}

#[test]
fn read_mem_config_via_backend_trait_folder_missing_returns_none() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir);
    let result = MemBackend::read_mem_config(&writer).unwrap();
    assert!(result.is_none());
}

#[test]
fn read_mem_config_via_backend_trait_archive_reads_bytes() {
    // Build an archive containing .memstead/config.json and verify
    // the ArchiveBackend impl returns its bytes.
    let tmp = TempDir::new().unwrap();
    let archive_path = tmp.path().join("seed.mem");
    let body = br#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": { "tone": "archive" }
        }"#;
    {
        let file = std::fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(
                ".memstead/config.json",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        use std::io::Write;
        writer.write_all(body).unwrap();
        writer.finish().unwrap();
    }

    let backend = ArchiveBackend::new(archive_path);
    let result = MemBackend::read_mem_config(&backend).unwrap();
    let bytes = result.expect("config bytes must surface");
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["writeGuidance"]["tone"], "archive");
}

#[test]
fn mem_config_for_returns_none_when_no_config_file_present() {
    // Folder backend without a `.memstead/config.json` file. The
    // accessor must lenient — return None, not error.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(engine.mem_config_for("specs").is_none());
}

#[test]
fn mem_config_for_returns_some_when_config_file_present() {
    // Drop a valid `.memstead/config.json` into the mem dir,
    // build the engine, and assert the accessor surfaces a
    // MemConfig with the right shape (write_guidance entries
    // round-trip).
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": {
                "tone": "neutral",
                "voice": "active"
            }
        }"#;
    std::fs::write(mem_dir.join(".memstead").join("config.json"), config_body).unwrap();

    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let cfg = engine
        .mem_config_for("specs")
        .expect("mem_config should load");
    assert_eq!(cfg.write_guidance.len(), 2);
    assert_eq!(
        cfg.write_guidance.get("tone").and_then(|v| v.as_str()),
        Some("neutral"),
    );
    assert_eq!(
        cfg.write_guidance.get("voice").and_then(|v| v.as_str()),
        Some("active"),
    );
}

#[test]
fn mem_config_for_unknown_mem_returns_none() {
    // Lenient accessor — unknown names get None, not Err.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(engine.mem_config_for("missing").is_none());
}

#[test]
fn mem_config_for_archive_mount_returns_none() {
    // Archive backends carry mem_config = None in V1 (the
    // read-from-storage path is deferred to a follow-up).
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
    let engine = Engine::from_mounts(vec![(
        archive_mount("ext", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(engine.mem_config_for("ext").is_none());
}

#[test]
fn mem_configs_named_iterates_only_mounts_with_config() {
    // Two folder mounts; one has a config file, one doesn't.
    // The iterator yields exactly the configured one — verifies
    // the filter_map shape and that the name comes from the
    // mount record (authoritative), not the config body.
    let tmp = TempDir::new().unwrap();
    let with_config = tmp.path().join("specs");
    let without_config = tmp.path().join("memos");
    std::fs::create_dir_all(with_config.join(".memstead")).unwrap();
    std::fs::create_dir_all(&without_config).unwrap();
    let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "writeGuidance": { "tone": "neutral" }
        }"#;
    std::fs::write(
        with_config.join(".memstead").join("config.json"),
        config_body,
    )
    .unwrap();

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("specs", with_config.clone()),
            Box::new(FilesystemBackend::new(with_config)) as Box<dyn MemBackend>,
        ),
        (
            folder_mount("memos", without_config.clone()),
            Box::new(FilesystemBackend::new(without_config)) as Box<dyn MemBackend>,
        ),
    ])
    .unwrap();

    let yielded: Vec<(&str, usize)> = engine
        .mem_configs_named()
        .map(|(name, cfg)| (name, cfg.write_guidance.len()))
        .collect();
    assert_eq!(yielded, vec![("specs", 1)]);
}

#[test]
fn schema_for_returns_some_for_known_mem_and_none_for_unknown() {
    // Every mount registers a schema (resolved from its pin at
    // boot). Lookup by mem name surfaces the same Arc that
    // mutations resolve internally; unknown names return None.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(engine.schema_for("specs").is_some());
    assert!(engine.schema_for("missing").is_none());
}

#[test]
fn gitdir_for_unknown_mem_returns_unknown_mem() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let err = engine.gitdir_for("missing").unwrap_err();
    assert!(matches!(err, EngineError::UnknownMem(v) if v == "missing"));
}

#[test]
fn gitdir_for_folder_mount_returns_no_gitdir_error() {
    // Folder mounts do not have a gitdir — full's contract surfaces
    // a mem-level error, not UnknownMem. Mirror that here.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let err = engine.gitdir_for("specs").unwrap_err();
    match err {
        EngineError::Mem(msg) => assert!(msg.contains("no resolved gitdir")),
        other => panic!("expected EngineError::Mem, got {other:?}"),
    }
}

#[test]
fn worktree_for_folder_mount_returns_storage_path() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let worktree = engine.worktree_for("specs").unwrap();
    assert_eq!(worktree, mem_dir);
}

#[test]
fn worktree_for_unknown_mem_returns_unknown_mem() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let err = engine.worktree_for("missing").unwrap_err();
    assert!(matches!(err, EngineError::UnknownMem(v) if v == "missing"));
}

#[test]
fn worktree_for_archive_mount_returns_archive_backed_error() {
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
    let engine = Engine::from_mounts(vec![(
        archive_mount("ext", archive_path.clone()),
        Box::new(ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let err = engine.worktree_for("ext").unwrap_err();
    match err {
        EngineError::Mem(msg) => assert!(msg.contains("archive-backed")),
        other => panic!("expected EngineError::Mem, got {other:?}"),
    }
}

#[test]
fn mem_head_sha_for_folder_mount_is_none() {
    // Folder backend doesn't track a head; current_head() returns
    // Ok(None) at construction; mem_head_sha returns Ok(None).
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let head = engine.mem_head_sha("specs").unwrap();
    assert_eq!(head, None);
}

#[test]
fn mem_head_sha_unknown_mem_returns_unknown_mem() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let err = engine.mem_head_sha("missing").unwrap_err();
    assert!(matches!(err, EngineError::UnknownMem(v) if v == "missing"));
}

#[test]
fn capability_surfaces_per_mount() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);

    let engine = Engine::from_mounts(vec![
        (
            folder_mount("writable", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        ),
        (
            archive_mount("read-only", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        ),
    ])
    .unwrap();

    assert_eq!(
        engine.capability("writable").unwrap(),
        MountCapability::Write
    );
    assert_eq!(
        engine.capability("read-only").unwrap(),
        MountCapability::ReadOnly
    );
    assert!(matches!(
        engine.capability("missing"),
        Err(EngineError::UnknownMem(_))
    ));
}

#[test]
fn read_provenance_routes_through_backend() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());

    // Append a provenance record via the backend trait directly,
    // then read it back through the engine.
    let backend_handle: &dyn MemBackend = &writer;
    backend_handle
        .append_provenance(&Provenance::new(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
            crate::ProvenanceKind::Create,
            Some("v:e".into()),
            crate::vcs::Actor::Cli,
            None,
            Some("first".into()),
        ))
        .unwrap();

    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let records = engine.read_provenance("specs", None).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, crate::ProvenanceKind::Create);
    assert_eq!(records[0].entity.as_deref(), Some("v:e"));
    assert_eq!(records[0].note.as_deref(), Some("first"));
}

#[test]
fn archive_mount_returns_sealed_indirectly_through_backend_layer() {
    // The engine doesn't yet expose mutation methods, but an
    // archive backend held on a Mount with ReadOnly capability is
    // still a `&dyn MemBackend` whose write methods return
    // Sealed. This test locks the trait routing — when the engine
    // gains write methods in a later session, capability gating +
    // backend Sealed errors must agree.
    let tmp = TempDir::new().unwrap();
    let archive_path = build_archive(tmp.path(), "ext", &[("a.md", b"a")]);
    let backend = ArchiveBackend::new(archive_path);
    match MemBackend::write_entity(&backend, Path::new("x.md"), b"x") {
        Err(BackendError::Sealed) => {}
        other => panic!("expected Sealed, got {other:?}"),
    }
}

// ---- Read-side delegates ----------------------------------------
//
// These tests pin the surface that the MCP migration consumes
// (stats, health, context, communities, search, list, orphans,
// stubs, most_connected, missing_required_outgoing). They run
// against a folder-mount engine with a small fixture of created
// entities and one relate edge — enough to exercise both the
// graph-query path and the cache-invalidation hooks.

/// Generation-keyed memos (flywheel W8/01). Four claims: repeated
/// reads serve the memo; a REFUSED engine batch leaves the memo
/// untouched (the refusal path calls no hook — pinned so it stays
/// cheap); a memo computed from an INTERIM (mid-batch) state never
/// survives a rollback as fresh — the store-carried generation is
/// what lets the invalidation hook adjudicate that correctly; and
/// a real mutation invalidates, with the recomputation identical
/// to a from-scratch detection (the criterion-3 identity oracle
/// for the mechanism).
#[test]
fn generation_keyed_memos_survive_rollback_and_track_mutations() {
    use indexmap::IndexMap;

    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);

    // 1. Repeated reads: cell filled once, generation stable.
    let _ = engine.communities();
    let memo_gen_before = engine.community_memo.get().expect("memo filled").0;
    let _ = engine.communities();
    assert_eq!(
        engine.community_memo.get().expect("still filled").0,
        memo_gen_before,
        "repeated reads must serve the memo"
    );

    // 2. A REFUSED engine batch rolls the store back and leaves
    // the memo untouched — refusal stays recompute-free.
    let bare = |id: crate::EntityId| crate::engine::UpdateEntityArgs {
        anchors: Vec::new(),
        anchors_unset: Vec::new(),
        id,
        expected_hash: None,
        sections: IndexMap::new(),
        append_sections: IndexMap::new(),
        patch_sections: IndexMap::new(),
        sections_unset: Vec::new(),
        metadata: IndexMap::new(),
        metadata_unset: Vec::new(),
        declare_relations: Vec::new(),
        dry_run: false,
        relations_unset: Vec::new(),
    };
    let mut real = bare(crate::EntityId::new("specs", "source-one"));
    real.append_sections
        .insert("identity".to_string(), "appended line".to_string());
    let missing = bare(crate::EntityId::new("specs", "does-not-exist"));
    let (actor, client) = cli_actor();
    let result = engine
        .batch_update(
            vec![(real, None), (missing, None)],
            actor,
            Some(&client),
            false,
        )
        .expect("refused batch returns a report-all envelope");
    assert!(
        !result.applied,
        "the missing target refuses the whole batch"
    );
    assert_eq!(
        engine.community_memo.get().map(|m| m.0),
        Some(memo_gen_before),
        "a refused batch must leave the pre-batch memo standing"
    );
    assert_eq!(
        engine.store().generation(),
        memo_gen_before.store_generation,
        "rollback restored the store to the memo's generation"
    );

    // 3. The dangerous direction: a memo computed from an INTERIM
    // state (simulated batch staging) must not survive the
    // rollback as fresh. The store-carried generation is what the
    // invalidation hook adjudicates with.
    let snapshot = engine.store.clone();
    let interim_id = crate::EntityId::new("specs", "interim-only");
    let mut interim = engine
        .store
        .get(&crate::EntityId::new("specs", "source-one"))
        .expect("demo entity present")
        .clone();
    interim.id = interim_id.clone();
    interim.title = "Interim Only".to_string();
    engine.store.upsert(interim_id, interim);
    engine.invalidate_communities();
    let _ = engine.communities();
    let interim_gen = engine.community_memo.get().expect("interim memo").0;
    assert_ne!(interim_gen, memo_gen_before);
    engine.store = snapshot; // rollback, generation restored with it
    engine.invalidate_communities();
    engine.invalidate_search_indexes();
    if let Some((g, _)) = engine.community_memo.get() {
        assert_ne!(
            *g, interim_gen,
            "a rolled-back interim state must never be served as fresh"
        );
    }
    assert!(
        !engine
            .communities()
            .entity_cluster_map
            .keys()
            .any(|id| id.contains("interim-only")),
        "the partition served after rollback reflects the restored store, not the interim one"
    );

    // 4. A real mutation invalidates; the recomputation equals a
    // from-scratch detection and sees the new entity.
    let (actor, client) = cli_actor();
    engine
        .create_entity(
            empty_create_args("specs", "Fourth Entity"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    assert!(
        engine
            .communities()
            .entity_cluster_map
            .keys()
            .any(|id| id.contains("fourth-entity")),
        "recomputed partition sees the new entity"
    );
    let fresh = {
        let schema = engine
            .schemas
            .iter()
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(_, s)| s.clone())
            .expect("demo engine has a schema");
        let schema_for_weights = schema.clone();
        crate::graph::community::detect_communities(
            engine.store(),
            schema.manifest.community.resolution,
            schema.manifest.community.seed,
            move |rel_type| {
                schema_for_weights
                    .manifest
                    .relationships
                    .definitions
                    .iter()
                    .find(|d| d.name == rel_type)
                    .map(|d| d.default_weight as f64)
                    .unwrap_or(1.0)
            },
        )
    };
    assert_eq!(
        engine.communities().entity_cluster_map,
        fresh.entity_cluster_map,
        "memo must equal a from-scratch detection over the current store"
    );
}

fn build_demo_engine(tmp: &TempDir) -> Engine {
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Source One"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let target = engine
        .create_entity(
            empty_create_args("specs", "Target Two"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    engine
        .create_entity(
            empty_create_args("specs", "Lonely Three"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: target.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    engine
}

#[test]
fn status_reports_per_engine_counts() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let stats = engine.status();
    assert_eq!(stats.entity_count, 3);
    assert_eq!(stats.edge_count, 1);
    assert_eq!(stats.mem_count, 1);
    assert_eq!(stats.types_in_use, vec!["spec".to_string()]);
    assert_eq!(stats.edge_types.get("USES"), Some(&1));
}

#[test]
fn orphans_lists_unconnected_real_entities() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let orphans = engine.orphans();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].as_ref(), "specs--lonely-three");
}

/// #49: the orphan/community headlines can be attributed per pinned
/// schema. Single-mem here, so one bucket — but it proves the
/// attribution keys by `schema_of(mem)` and that the per-schema
/// counts sum to the raw total (which a health surface keeps verbatim).
#[test]
fn schema_breakdowns_attribute_to_mem_pin() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);

    let orphans = engine.orphans();
    let orphans_by_schema = engine.orphans_by_schema(&orphans);
    assert_eq!(
        orphans_by_schema.values().sum::<usize>(),
        orphans.len(),
        "per-schema orphan counts must sum to the raw total"
    );
    assert_eq!(orphans_by_schema.len(), 1, "one mem ⇒ one schema bucket");
    let (schema, count) = orphans_by_schema.iter().next().unwrap();
    assert!(!schema.is_empty(), "specs mem is pinned: {schema:?}");
    assert_eq!(*count, 1);

    // communities_by_schema buckets the demo mem's clusters under the
    // same pin; with one schema, its values sum to the global count.
    let mems: Vec<String> = engine.mounts().iter().map(|m| m.mem.clone()).collect();
    let communities_by_schema = engine.communities_by_schema(&mems);
    assert_eq!(communities_by_schema.len(), 1);
    assert_eq!(
        communities_by_schema.values().sum::<usize>(),
        engine.communities().count,
    );
}

#[test]
fn stubs_lists_unresolved_link_targets() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let source = engine
        .create_entity(
            empty_create_args("specs", "Holder"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // Relate to a non-existent target — relate_entity creates a
    // stub for the target so the edge can land.
    engine
        .relate_entity(
            RelateEntityArgs {
                source: source.id.clone(),
                expected_hash: Some(source.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: EntityId::new("specs", "ghost"),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let stubs = engine.stubs();
    assert!(
        stubs.iter().any(|(id, _)| id.as_ref() == "specs--ghost"),
        "expected ghost stub: {stubs:?}"
    );
}

#[test]
fn most_connected_orders_by_degree() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let top = engine.most_connected(5);
    assert_eq!(top.len(), 3);
    // Source and Target each have one edge; Lonely has zero.
    let zero_degree: Vec<_> = top
        .iter()
        .filter(|c| c.total == 0)
        .map(|c| c.id.as_ref().to_string())
        .collect();
    assert_eq!(zero_degree, vec!["specs--lonely-three".to_string()]);
}

#[test]
fn health_returns_per_engine_summary() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let health = engine.health();
    // `memstead_create` refuses on missing required sections, so
    // entities built through `empty_create_args` carry the
    // helper-seeded `identity` + `purpose` bodies and no longer
    // surface as missing-fields. Health remains the read-side
    // tolerance surface for legacy on-disk drift — covered by
    // the loader-tolerance tests that hand-craft pre-strict
    // markdown files.
    assert!(
        health
            .missing_fields
            .iter()
            .all(|r| r.id.as_ref() != "specs--source-one"),
        "post-strict-create fixture must not surface as missing-fields; got {:?}",
        health.missing_fields,
    );
}

#[test]
fn context_carries_neighbors_and_community() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let source_id = EntityId::new("specs", "source-one");
    let ctx = engine.context(&source_id).unwrap();
    assert_eq!(ctx.entity_id, source_id);
    assert_eq!(ctx.neighbors.len(), 1);
    assert_eq!(ctx.neighbors[0].relationship, "USES");
    assert!(matches!(ctx.neighbors[0].direction, Direction::Outgoing));
}

#[test]
fn communities_caches_louvain_until_invalidated() {
    let tmp = TempDir::new().unwrap();
    let mut engine = build_demo_engine(&tmp);
    // Population reflects the current store at first call.
    let entities_before = engine.communities().entity_cluster_map.len();
    // Cache hit — repeat call returns same data.
    assert_eq!(
        engine.communities().entity_cluster_map.len(),
        entities_before
    );
    // Mutation invalidates the cache; next call re-runs against
    // the post-mutation store and includes the new entity.
    let (actor, client) = cli_actor();
    engine
        .create_entity(
            empty_create_args("specs", "Disturber"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let entities_after = engine.communities().entity_cluster_map.len();
    assert_eq!(
        entities_after,
        entities_before + 1,
        "create_entity should have invalidated community cache and added the new entity"
    );
}

#[test]
fn list_filters_by_metadata_only() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let scope = SearchScope {
        entity_type: Some("spec".to_string()),
        ..Default::default()
    };
    let result = engine.list(&scope);
    // Three real spec entities created; stubs / non-spec types absent.
    assert_eq!(result.hits.len(), 3);
}

#[test]
fn list_applies_schema_declared_filter_on_non_default_schema_mem() {
    // A mem pinned to `planning` (non-default schema). The
    // `decision` type declares `status` with `filterable: equality`.
    // Pre-fix, filter dispatch consulted only the built-in default
    // schema via `type_by_name`, missed `status`, silently bypassed
    // the filter, and emitted the misleading "unknown filter key"
    // warning. Post-fix, the filter is honored and no warning fires.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().to_path_buf();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mount = Mount {
        mem: "planning".to_string(),
        schema: Some(memstead_schema::SchemaRef::new(
            "planning",
            semver::Version::new(0, 1, 0),
        )),
        storage: MountStorage::Folder { path: mem_dir },
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: true,
        migration_target: None,
    };
    let mut engine =
        Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
    let (actor, client) = cli_actor();

    // Two decisions with different status values; required fields
    // (decision/context/consequences sections, decided_on, deciders)
    // get placeholder defaults — the test only cares about the
    // status field's filterability.
    for (title, status) in &[("Skip Postgres", "accepted"), ("Use SQLite", "proposed")] {
        let mut metadata = indexmap::IndexMap::new();
        metadata.insert("status".to_string(), status.to_string());
        metadata.insert("deciders".to_string(), "alice".to_string());
        metadata.insert("decided_on".to_string(), "2026-05-19".to_string());
        let args = crate::engine::CreateEntityArgs {
            anchors: Vec::new(),
            mem: "planning".to_string(),
            title: title.to_string(),
            entity_type: "decision".to_string(),
            sections: indexmap::IndexMap::from_iter([
                ("decision".to_string(), "We chose this.".to_string()),
                ("context".to_string(), "Single-user dev.".to_string()),
                ("consequences".to_string(), "Lose multi-writer.".to_string()),
            ]),
            metadata,
            relations: Vec::new(),
            dry_run: false,
        };
        engine
            .create_entity(args, actor, Some(&client), None)
            .unwrap();
    }

    // Filter on the schema-declared filterable field.
    let scope = SearchScope {
        entity_type: Some("decision".to_string()),
        filters: std::collections::HashMap::from([("status".to_string(), "accepted".to_string())]),
        ..Default::default()
    };
    let result = engine.list(&scope);
    assert_eq!(
        result.hits.len(),
        1,
        "filter on schema-declared field must select only matching entities"
    );
    assert_eq!(result.hits[0].title, "Skip Postgres");
    assert!(
        result.warnings.is_empty(),
        "no warning should fire when the filter is declared by the mem's pinned schema: {:?}",
        result.warnings
    );
}

#[test]
fn search_returns_results_against_built_index() {
    let tmp = TempDir::new().unwrap();
    let engine = build_demo_engine(&tmp);
    let scope = SearchScope {
        query: Some(crate::ops::Query {
            any: vec!["source".to_string()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = engine.search(&scope).expect("native search returns Ok");
    assert!(result.total >= 1, "expected ≥1 hit for source: {result:?}");
    assert!(
        result
            .hits
            .iter()
            .any(|h| h.id.as_ref() == "specs--source-one"),
        "expected source-one in hits: {result:?}"
    );
}

// ---- Engine::from_workspace_root (folder boot path) ------------
