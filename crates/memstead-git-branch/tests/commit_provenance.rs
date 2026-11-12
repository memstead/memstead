//! Every commit the engine writes carries the mutation's provenance:
//! the `Tool:`, `Actor:`, `Client:`, `Role:` and `Identity:` trailers
//! and the note. One workspace, every commit-producing operation the
//! lifecycle and the maintenance loop reach, and every commit the
//! mem-repo holds afterwards read back and checked. Until 2026-09-11
//! the config-write commit on the schema-and-config ref, the binding
//! store's edits, the force-overwrite prune and the mem rename built
//! contexts of their own with no role, no identity and no client, and
//! the CLI's `projection init` / `enable` / `edit` wrote the binding
//! record with no commit at all.

use memstead_base::mem_management::{self, StorageKind};
use memstead_base::vcs::{Actor, ClientId, Role};
use memstead_git_branch::test_support::init_real_mem_repo;
use memstead_git_branch::workspace_store::engine_from_workspace_root;
use tempfile::TempDir;

const BINDING_PATCH: &str = r#"{
    "sources": [{ "name": "codebase", "type": "codebase", "pointer": "src/",
                  "scope": [{ "path": "**/*", "mode": "allow" }] }],
    "destination_mem": "specs"
}"#;

/// Every commit reachable from every ref of the mem-repo, as
/// `(id, subject, full message)`.
fn all_commits(workspace_root: &std::path::Path) -> Vec<(String, String, String)> {
    let repo = gix::open(workspace_root.join("mem-repo").join(".git")).expect("open mem-repo");
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for reference in repo.references().expect("refs").all().expect("iter") {
        let reference = reference.expect("ref");
        let Ok(tip) = reference.into_fully_peeled_id() else {
            continue;
        };
        let walk = repo.rev_walk([tip.detach()]).all().expect("rev walk");
        for info in walk {
            let info = info.expect("commit info");
            if !seen.insert(info.id) {
                continue;
            }
            let commit = repo.find_object(info.id).expect("object").into_commit();
            let message =
                String::from_utf8_lossy(commit.message_raw().expect("message")).to_string();
            let subject = message.lines().next().unwrap_or("").to_string();
            out.push((info.id.to_string(), subject, message));
        }
    }
    out
}

fn create_params(name: &str, note: &str) -> mem_management::MemCreateParams {
    mem_management::MemCreateParams {
        name: name.to_string(),
        location: std::path::PathBuf::from(name),
        schema_ref: "default@1.0.0".parse().unwrap(),
        vcs: None,
        note: Some(note.to_string()),
        operator_mode: true,
        recovery: None,
        write_guidance: Default::default(),
        storage: Some(StorageKind::GitBranch),
        actor: Actor::Cli,
        client: Some(ClientId {
            name: "provenance-test".to_string(),
            version: "0".to_string(),
        }),
    }
}

#[test]
fn every_commit_the_engine_writes_carries_the_sessions_provenance() {
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let baseline: std::collections::HashSet<String> = all_commits(tmp.path())
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();

    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    engine.set_role(Role::Author);
    engine.set_identity(Some("session-42".to_string()));
    engine.set_actor(Actor::Cli);
    engine.set_client(Some(ClientId {
        name: "provenance-test".to_string(),
        version: "0".to_string(),
    }));

    // A create: the seed commit on the mem's branch and the config
    // blob on the schema-and-config ref.
    mem_management::create_mem(&mut engine, create_params("noted", "created for the pin"))
        .expect("create");
    // Config writes the session causes as itself.
    engine
        .set_mem_version(
            "noted",
            "0.2.0".parse().unwrap(),
            Some("bumped for the pin"),
        )
        .expect("set version");
    engine
        .set_mem_title(
            "noted",
            Some("Noted".to_string()),
            Some("titled for the pin"),
        )
        .expect("set title");
    engine
        .set_mem_sync_state(
            "specs",
            "specs/graph/codebase#synced",
            "abc123",
            Some("stamped"),
        )
        .expect("sync state");
    // A binding-store edit.
    engine
        .add_projection_json("specs", "graph", BINDING_PATCH, Some("bound for the pin"))
        .expect("binding add");
    // An entity mutation, for comparison.
    let mut sections = indexmap::IndexMap::new();
    sections.insert("identity".to_string(), "An entity.".to_string());
    sections.insert("purpose".to_string(), "Carries an anchor.".to_string());
    // The anchored artifact must resolve under the workspace root.
    std::fs::create_dir_all(tmp.path().join("src")).expect("src dir");
    std::fs::write(tmp.path().join("src").join("lib.rs"), "pub fn lib() {}\n").expect("lib.rs");
    let created = engine
        .create_entity(
            memstead_base::CreateEntityArgs {
                mem: "specs".to_string(),
                title: "Anchored".to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata: Default::default(),
                relations: Vec::new(),
                dry_run: false,
                anchors: vec![memstead_base::anchor::AnchorInput {
                    artifact: Some("src/lib.rs".to_string()),
                    grain: Some("file".to_string()),
                    class: Some("anchored".to_string()),
                    hash_stability: Some("stable".to_string()),
                    ..Default::default()
                }],
            },
            Actor::Cli,
            None,
            Some("an entity"),
        )
        .expect("create entity");
    // The maintenance loop's anchor backfill: the observed hash lands
    // on the sidecar in a commit of its own.
    let written = engine
        .record_anchor_observed_hashes(
            "specs",
            &[memstead_base::anchor::ObservedArtifactHash {
                entity: created.id.to_string(),
                artifact: "src/lib.rs".to_string(),
                hash: "observed-1".to_string(),
            }],
            Some("backfilled"),
        )
        .expect("anchor backfill");
    assert_eq!(written, 1, "the backfill must reach the anchored entity");
    // A deletion: the prune commit.
    mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "noted".to_string(),
            delete_files: true,
            note: Some("retired for the pin".to_string()),
            actor: Actor::Cli,
            client: None,
            operator_mode: true,
            detach_incoming: false,
        },
    )
    .expect("delete");
    // A force-overwrite create over a leftover branch: the residue prune.
    mem_management::create_mem(&mut engine, create_params("twice", "first")).expect("create twice");
    mem_management::delete_mem(
        &mut engine,
        mem_management::MemDeleteParams {
            name: "twice".to_string(),
            delete_files: false,
            note: Some("unregistered, branch kept".to_string()),
            actor: Actor::Cli,
            client: None,
            operator_mode: true,
            detach_incoming: false,
        },
    )
    .expect("unregister twice");
    let mut again = create_params("twice", "recreated over the residue");
    again.recovery = Some(memstead_base::RecoveryAction::ForceOverwrite);
    mem_management::create_mem(&mut engine, again).expect("force overwrite");

    // Every commit the session wrote, none of the fixture's.
    let commits: Vec<(String, String)> = all_commits(tmp.path())
        .into_iter()
        .filter(|(id, _, _)| !baseline.contains(id))
        .map(|(_, s, m)| (s, m))
        .collect();
    assert!(
        commits.len() >= 9,
        "expected at least nine new commits, got {}: {:?}",
        commits.len(),
        commits.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>()
    );
    for (subject, message) in &commits {
        for trailer in [
            "Tool: ",
            "Actor: cli",
            "Role: author",
            "Identity: session-42",
        ] {
            assert!(
                message.contains(trailer),
                "commit {subject:?} lacks {trailer:?}:\n{message}"
            );
        }
    }
    // The notes ride the bodies where they were given.
    for note in [
        "created for the pin",
        "bumped for the pin",
        "titled for the pin",
        "stamped",
        "bound for the pin",
        "an entity",
        "backfilled",
        "retired for the pin",
        "recreated over the residue",
    ] {
        assert!(
            commits.iter().any(|(_, m)| m.contains(note)),
            "no commit carries the note {note:?}"
        );
    }
}

/// A batch's entities each read their own provenance in the exported
/// archive: under their own id, with the note of their own entry, with
/// the batch's kind (`create` for a batch create, `update` for a batch
/// update). No record is keyed by the batch subject's pseudo-key.
#[test]
fn a_batch_written_entitys_archive_provenance_is_its_own() {
    use indexmap::IndexMap;
    let tmp = TempDir::new().unwrap();
    init_real_mem_repo(tmp.path(), &[("specs", "default@1.0.0")]);
    let mut engine = engine_from_workspace_root(tmp.path()).expect("engine boots");
    engine
        .set_mem_version("specs", semver::Version::new(1, 0, 0), None)
        .expect("version set");
    let create = |title: &str| {
        let mut sections = IndexMap::new();
        sections.insert("identity".to_string(), format!("{title} identity"));
        sections.insert("purpose".to_string(), format!("{title} purpose"));
        memstead_base::CreateEntityArgs {
            anchors: Vec::new(),
            mem: "specs".to_string(),
            title: title.to_string(),
            entity_type: "spec".to_string(),
            sections,
            metadata: IndexMap::new(),
            relations: Vec::new(),
            dry_run: false,
        }
    };
    engine
        .batch_create(
            vec![
                (create("Alpha"), Some("alpha was made".to_string())),
                (create("Beta"), Some("beta was made".to_string())),
            ],
            Actor::Cli,
            None,
            false,
        )
        .expect("batch create lands");

    let provenance = |engine: &memstead_base::Engine| -> serde_json::Value {
        let out = tmp.path().join("specs.mem");
        engine.export_mem("specs", &out).expect("export");
        let bytes = std::fs::read(&out).unwrap();
        let entries = memstead_base::validator::archive::extract_entries(
            &bytes,
            &memstead_base::validator::ValidatorLimits::DEFAULT,
        )
        .unwrap();
        serde_json::from_slice(&entries.provenance_bytes.expect("provenance member")).unwrap()
    };
    let after_create = provenance(&engine);
    let keys: Vec<&String> = after_create["entities"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(
        keys,
        ["alpha", "beta"],
        "no record is keyed by a subject: {after_create}"
    );
    let alpha = find_record(&after_create, "alpha");
    assert_eq!(alpha["rationale"], "alpha was made", "{alpha}");
    assert_eq!(alpha["kind"], "create", "{alpha}");
    let beta = find_record(&after_create, "beta");
    assert_eq!(beta["rationale"], "beta was made", "{beta}");
    assert_eq!(beta["kind"], "create", "{beta}");

    let update = |engine: &memstead_base::Engine, slug: &str| {
        let id = memstead_base::EntityId::new("specs", slug);
        let mut sections = IndexMap::new();
        sections.insert("purpose".to_string(), format!("{slug} purpose, updated"));
        memstead_base::UpdateEntityArgs {
            expected_hash: memstead_base::ExpectedHash::from_token(
                engine.get_entity(&id).map(|e| e.content_hash.clone()),
            ),
            id,
            sections,
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            sections_unset: Vec::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            dry_run: false,
            declare_relations: Vec::new(),
            relations_unset: Vec::new(),
            anchors: Vec::new(),
            anchors_unset: Vec::new(),
        }
    };
    // Two entities created singly and without a note, so the batch
    // update's records are their only noted ones: the archive keeps the
    // newest noted record per entity, and commit times have one-second
    // resolution, so a create and an update in one second would tie.
    for title in ["Gamma", "Delta"] {
        engine
            .create_entity(create(title), Actor::Cli, None, None)
            .expect("create lands");
    }
    let updates = vec![
        (
            update(&engine, "gamma"),
            Some("gamma was revised".to_string()),
        ),
        (
            update(&engine, "delta"),
            Some("delta was revised".to_string()),
        ),
    ];
    engine
        .batch_update(updates, Actor::Cli, None, false)
        .expect("batch update lands");
    let after_update = provenance(&engine);
    let keys: Vec<&String> = after_update["entities"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(
        keys,
        ["alpha", "beta", "delta", "gamma"],
        "no record is keyed by a subject: {after_update}"
    );
    let gamma = find_record(&after_update, "gamma");
    assert_eq!(gamma["rationale"], "gamma was revised", "{gamma}");
    assert_eq!(gamma["kind"], "update", "{gamma}");
    let delta = find_record(&after_update, "delta");
    assert_eq!(delta["rationale"], "delta was revised", "{delta}");
    assert_eq!(delta["kind"], "update", "{delta}");
}

/// The archive provenance record of entity `slug` (the payload keys its
/// records by the entity's mem-relative id under `entities`).
fn find_record(payload: &serde_json::Value, slug: &str) -> serde_json::Value {
    payload["entities"]
        .get(slug)
        .cloned()
        .unwrap_or_else(|| panic!("no record for {slug} in {payload}"))
}
