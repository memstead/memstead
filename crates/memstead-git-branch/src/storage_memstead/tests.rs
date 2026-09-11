#![cfg(test)]

use super::*;
use std::path::PathBuf;
use tempfile::TempDir;

fn fresh_repo_dir(tmp: &Path) -> PathBuf {
    let git_dir = tmp.join("mem-repo.git");
    gix::init_bare(&git_dir).unwrap();
    std::fs::canonicalize(&git_dir).unwrap()
}

fn actor_for_test() -> gix::actor::Signature {
    gix::actor::Signature {
        name: "test".into(),
        email: "test@example.com".into(),
        time: gix::date::Time {
            seconds: 0,
            offset: 0,
        },
    }
}

/// `(name, version, [(filename, body)])` schema seed tuple.
type SchemaSeed<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

/// Build a minimal __SCHEMAS commit with one schema directory
/// `<name>` containing `schema.yaml` (with a `version: <v>` field)
/// and an optional types subtree of `(filename, body)` pairs.
fn seed_schemas(gitdir: &Path, schemas: &[SchemaSeed<'_>]) {
    let repo = gix::open(gitdir).unwrap();
    let actor = actor_for_test();
    let mut buf = gix::date::parse::TimeBuf::default();
    let sig_ref = actor.to_ref(&mut buf);
    let mut editor = repo.empty_tree().edit().unwrap();
    for (name, version, types) in schemas {
        let manifest = format!("name: {name}\nversion: \"{version}\"\n");
        let manifest_blob = repo.write_blob(manifest.as_bytes()).unwrap().detach();
        editor
            .upsert(
                format!("{name}/schema.yaml"),
                gix::object::tree::EntryKind::Blob,
                manifest_blob,
            )
            .unwrap();
        for (type_name, type_body) in *types {
            let blob = repo.write_blob(type_body.as_bytes()).unwrap().detach();
            editor
                .upsert(
                    format!("{name}/types/{type_name}.yaml"),
                    gix::object::tree::EntryKind::Blob,
                    blob,
                )
                .unwrap();
        }
    }
    let tree_id = editor.write().unwrap().detach();
    repo.commit_as(
        sig_ref,
        sig_ref,
        "refs/heads/__SCHEMAS",
        "seed __SCHEMAS",
        tree_id,
        Vec::<gix::ObjectId>::new(),
    )
    .unwrap();
}

/// Build a minimal __SYSTEM commit. `mems` is `(mem_name,
/// config_json)` pairs; `repo_json` is the repo.json blob (or
/// empty to skip).
fn seed_system(gitdir: &Path, repo_json: &str, mems: &[(&str, &str)]) {
    let repo = gix::open(gitdir).unwrap();
    let actor = actor_for_test();
    let mut buf = gix::date::parse::TimeBuf::default();
    let sig_ref = actor.to_ref(&mut buf);
    let mut editor = repo.empty_tree().edit().unwrap();
    if !repo_json.is_empty() {
        let blob = repo.write_blob(repo_json.as_bytes()).unwrap().detach();
        editor
            .upsert("repo.json", gix::object::tree::EntryKind::Blob, blob)
            .unwrap();
    }
    for (mem, config) in mems {
        let blob = repo.write_blob(config.as_bytes()).unwrap().detach();
        editor
            .upsert(
                format!("{mem}/config.json"),
                gix::object::tree::EntryKind::Blob,
                blob,
            )
            .unwrap();
    }
    let tree_id = editor.write().unwrap().detach();
    repo.commit_as(
        sig_ref,
        sig_ref,
        "refs/heads/__SYSTEM",
        "seed __SYSTEM",
        tree_id,
        Vec::<gix::ObjectId>::new(),
    )
    .unwrap();
}

/// Walk the `__MEMSTEAD` tree at `gitdir` and return every
/// (path, blob_oid) entry — used to assert tree shape.
fn list_memstead_entries(gitdir: &Path) -> Vec<String> {
    let repo = gix::open(gitdir).unwrap();
    let reference = repo
        .try_find_reference("refs/heads/__MEMSTEAD")
        .unwrap()
        .unwrap();
    let id = reference.into_fully_peeled_id().unwrap();
    let commit = id.object().unwrap().try_into_commit().unwrap();
    let tree = commit.tree().unwrap();
    let mut out: Vec<String> = Vec::new();
    walk(&repo, &tree, "", &mut out);
    out.sort();
    out
}

fn walk(repo: &gix::Repository, tree: &gix::Tree<'_>, prefix: &str, out: &mut Vec<String>) {
    for entry in tree.iter().flatten() {
        let name = std::str::from_utf8(entry.filename())
            .unwrap_or("")
            .to_string();
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.mode().kind() {
            gix::object::tree::EntryKind::Tree => {
                let subtree = repo
                    .find_object(entry.oid().to_owned())
                    .unwrap()
                    .into_tree();
                walk(repo, &subtree, &path, out);
            }
            gix::object::tree::EntryKind::Blob | gix::object::tree::EntryKind::BlobExecutable => {
                out.push(path);
            }
            _ => {}
        }
    }
}

#[test]
fn migrate_writes_unified_tree_from_schemas_and_system() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    seed_schemas(
        &gitdir,
        &[
            ("default", "1.0.0", &[("spec", "name: spec\n")]),
            ("custom", "0.5.0", &[]),
        ],
    );
    seed_system(
        &gitdir,
        r#"{"name":"main"}"#,
        &[
            ("alpha", r#"{"format": 1, "schema": "default@1.0.0"}"#),
            ("beta", r#"{"format": 1, "schema": "custom@0.5.0"}"#),
        ],
    );

    let outcome = migrate_to_memstead_ref(&gitdir).unwrap();
    assert_eq!(outcome.schemas_migrated, 2);
    assert_eq!(outcome.mems_migrated, 2);
    assert!(!outcome.already_current);

    let entries = list_memstead_entries(&gitdir);
    // Schemas live under versioned paths.
    assert!(entries.contains(&"schemas/default@1.0.0/schema.yaml".to_string()));
    assert!(entries.contains(&"schemas/default@1.0.0/types/spec.yaml".to_string()));
    assert!(entries.contains(&"schemas/custom@0.5.0/schema.yaml".to_string()));
    // Mem configs live under mems/.
    assert!(entries.contains(&"mems/alpha/config.json".to_string()));
    assert!(entries.contains(&"mems/beta/config.json".to_string()));
    // repo.json explicitly NOT migrated.
    assert!(
        !entries.iter().any(|p| p.contains("repo.json")),
        "repo.json must not appear under __MEMSTEAD: {entries:?}"
    );
}

/// The format marker decides the sealed reader's metadata-polarity
/// generation: the same package sealed WITHOUT the marker reads an
/// absent required/optional key as required (legacy written
/// meaning), and sealed WITH the marker (the install helper) as
/// optional.
#[test]
fn sealed_format_marker_decides_metadata_polarity() {
    let manifest = b"name: tiny\nversion: 0.1.0\ndescription: t\nwhen_to_use: tests\ntypes:\n  - doc\nrelationships:\n  mode: strict\n  definitions:\n    - name: PART_OF\n      description: h\n      default_weight: 3.0\n    - name: _default\n      description: d\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n".to_vec();
    let doc = b"name: doc\ndescription: t\nwhen_to_use: tests\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: bare\n    description: no required key\n    field_type: string\ntitle_weight: 100.0\ntext_fields: [body]\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields: [title, body]\nhealth_required_fields: []\nstaleness_threshold_days: 90\nwrite_rules: []\n".to_vec();
    let files = vec![
        ("schema.yaml".to_string(), manifest),
        ("types/doc.yaml".to_string(), doc),
    ];

    // Unmarked seal (a package sealed before the polarity flip).
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    write_schema_to_memstead_ref(&gitdir, "tiny", "0.1.0", &files).unwrap();
    let LoadOutcome::Schemas(schemas) = load_schemas_from_memstead_ref_at_gitdir(&gitdir).unwrap()
    else {
        panic!("schemas expected");
    };
    assert!(
        schemas[0]
            .get_type("doc")
            .unwrap()
            .metadata_field("bare")
            .unwrap()
            .is_required(),
        "unmarked sealed package: absence keeps the legacy meaning (required)"
    );

    // Marked seal — the install helper appends the marker.
    let tmp2 = TempDir::new().unwrap();
    let gitdir2 = fresh_repo_dir(tmp2.path());
    let marked = memstead_schema::loader::with_format_marker(files);
    write_schema_to_memstead_ref(&gitdir2, "tiny", "0.1.0", &marked).unwrap();
    let LoadOutcome::Schemas(schemas) = load_schemas_from_memstead_ref_at_gitdir(&gitdir2).unwrap()
    else {
        panic!("schemas expected");
    };
    assert!(
        !schemas[0]
            .get_type("doc")
            .unwrap()
            .metadata_field("bare")
            .unwrap()
            .is_required(),
        "marked sealed package: absence means optional"
    );
}

#[test]
fn write_schema_to_memstead_ref_adds_package_idempotent_and_preserves_others() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());

    // Write a package onto an absent ref — the ref is created.
    let tiny = vec![
        (
            "schema.yaml".to_string(),
            b"name: tiny\nversion: 0.1.0\n".to_vec(),
        ),
        ("types/doc.yaml".to_string(), b"name: doc\n".to_vec()),
        ("mem-template.json".to_string(), b"{}\n".to_vec()),
    ];
    let out = write_schema_to_memstead_ref(&gitdir, "tiny", "0.1.0", &tiny).unwrap();
    assert!(!out.already_current);
    let entries = list_memstead_entries(&gitdir);
    for p in [
        "schemas/tiny@0.1.0/schema.yaml",
        "schemas/tiny@0.1.0/types/doc.yaml",
        "schemas/tiny@0.1.0/mem-template.json",
    ] {
        assert!(entries.iter().any(|e| e == p), "missing {p}: {entries:?}");
    }

    // Re-writing identical bytes is a no-op — same tip, no new commit.
    let again = write_schema_to_memstead_ref(&gitdir, "tiny", "0.1.0", &tiny).unwrap();
    assert!(again.already_current);
    assert_eq!(out.commit_sha, again.commit_sha);

    // A second package upserts into the existing tree, preserving the first.
    let other = vec![(
        "schema.yaml".to_string(),
        b"name: other\nversion: 2.0.0\n".to_vec(),
    )];
    let out2 = write_schema_to_memstead_ref(&gitdir, "other", "2.0.0", &other).unwrap();
    assert!(!out2.already_current);
    assert_ne!(out2.commit_sha, out.commit_sha);
    let entries2 = list_memstead_entries(&gitdir);
    assert!(
        entries2
            .iter()
            .any(|e| e == "schemas/tiny@0.1.0/schema.yaml")
    );
    assert!(
        entries2
            .iter()
            .any(|e| e == "schemas/other@2.0.0/schema.yaml")
    );
}

/// `read_schema_file_from_memstead_ref` round-trips a file written
/// by `write_schema_to_memstead_ref` (the install-provenance stamp
/// being the consumer), and absence — of the file, the package, or
/// the ref — is `Ok(None)`, never an error.
#[test]
fn read_schema_file_round_trips_and_absence_is_none() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());

    // Absent ref → None.
    assert!(
        read_schema_file_from_memstead_ref(&gitdir, "tiny", "0.1.0", "install-provenance.json")
            .unwrap()
            .is_none()
    );

    let stamp = br#"{"authoring_path":"/tmp/somewhere/tiny"}"#.to_vec();
    let files = vec![
        (
            "schema.yaml".to_string(),
            b"name: tiny\nversion: 0.1.0\n".to_vec(),
        ),
        ("install-provenance.json".to_string(), stamp.clone()),
    ];
    write_schema_to_memstead_ref(&gitdir, "tiny", "0.1.0", &files).unwrap();

    // Present file round-trips byte-exact (via the ops hook too).
    assert_eq!(
        read_schema_file_from_memstead_ref(&gitdir, "tiny", "0.1.0", "install-provenance.json")
            .unwrap()
            .as_deref(),
        Some(stamp.as_slice())
    );
    assert_eq!(
        (crate::storage::FULL_GIT_BRANCH_OPS.read_schema_file)(
            &gitdir,
            "tiny",
            "0.1.0",
            "install-provenance.json"
        )
        .unwrap()
        .as_deref(),
        Some(stamp.as_slice())
    );

    // Absent file in a present package → None; absent package → None.
    assert!(
        read_schema_file_from_memstead_ref(&gitdir, "tiny", "0.1.0", "no-such-file.json")
            .unwrap()
            .is_none()
    );
    assert!(
        read_schema_file_from_memstead_ref(&gitdir, "ghost", "9.9.9", "install-provenance.json")
            .unwrap()
            .is_none()
    );
}

#[test]
fn full_git_branch_ops_write_schema_hook_writes_to_ref() {
    // The engine reaches the ref-write through the
    // `GitBranchOps.write_schema` dispatcher; this pins that the const
    // is wired to `write_schema_to_memstead_ref` and returns a sha.
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let files = vec![(
        "schema.yaml".to_string(),
        b"name: h\nversion: 1.0.0\n".to_vec(),
    )];
    let commit = (crate::storage::FULL_GIT_BRANCH_OPS.write_schema)(&gitdir, "h", "1.0.0", &files)
        .expect("hook writes the package");
    assert!(!commit.is_empty());
    let entries = list_memstead_entries(&gitdir);
    assert!(
        entries.iter().any(|e| e == "schemas/h@1.0.0/schema.yaml"),
        "package must land on the ref: {entries:?}"
    );
}

#[test]
fn migrate_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    seed_schemas(&gitdir, &[("default", "1.0.0", &[])]);
    seed_system(
        &gitdir,
        r#"{"name":"main"}"#,
        &[("alpha", r#"{"format": 1, "schema": "default@1.0.0"}"#)],
    );

    let first = migrate_to_memstead_ref(&gitdir).unwrap();
    assert!(!first.already_current);
    let second = migrate_to_memstead_ref(&gitdir).unwrap();
    assert!(second.already_current);
    // Same tip; no new commit was written.
    assert_eq!(first.commit_sha, second.commit_sha);
}

#[test]
fn migrate_with_empty_workspace_writes_empty_tree() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    // No __SCHEMAS, no __SYSTEM — the migration writes an
    // empty __MEMSTEAD tree (the cutover session decides what to
    // do with that case).
    let outcome = migrate_to_memstead_ref(&gitdir).unwrap();
    assert_eq!(outcome.schemas_migrated, 0);
    assert_eq!(outcome.mems_migrated, 0);
    let entries = list_memstead_entries(&gitdir);
    assert!(entries.is_empty());
}

#[test]
fn migrate_handles_missing_version_with_placeholder() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    // Schema YAML with no `version:` field.
    let repo = gix::open(&gitdir).unwrap();
    let actor = actor_for_test();
    let mut buf = gix::date::parse::TimeBuf::default();
    let sig_ref = actor.to_ref(&mut buf);
    let mut editor = repo.empty_tree().edit().unwrap();
    let blob = repo.write_blob(b"name: anonymous\n").unwrap().detach();
    editor
        .upsert(
            "anonymous/schema.yaml",
            gix::object::tree::EntryKind::Blob,
            blob,
        )
        .unwrap();
    let tree_id = editor.write().unwrap().detach();
    repo.commit_as(
        sig_ref,
        sig_ref,
        "refs/heads/__SCHEMAS",
        "seed",
        tree_id,
        Vec::<gix::ObjectId>::new(),
    )
    .unwrap();

    let outcome = migrate_to_memstead_ref(&gitdir).unwrap();
    assert_eq!(outcome.schemas_migrated, 1);
    let entries = list_memstead_entries(&gitdir);
    assert!(
        entries.contains(&"schemas/anonymous@0.0.0/schema.yaml".to_string()),
        "missing-version schemas land under @0.0.0; got {entries:?}"
    );
}

#[test]
fn read_mem_config_from_memstead_round_trips_after_migration() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    seed_schemas(&gitdir, &[("default", "1.0.0", &[])]);
    seed_system(
        &gitdir,
        r#"{"name":"main"}"#,
        &[("alpha", r#"{"format": 1, "schema": "default@1.0.0"}"#)],
    );
    let _ = migrate_to_memstead_ref(&gitdir).unwrap();

    let config = read_mem_config_from_memstead_ref(&gitdir, "alpha").unwrap();
    assert!(config.schema.is_some());
    assert_eq!(config.schema.unwrap().to_string(), "default@1.0.0");
}

#[test]
fn read_mem_config_from_memstead_returns_typed_error_for_missing_mem() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    seed_schemas(&gitdir, &[("default", "1.0.0", &[])]);
    seed_system(
        &gitdir,
        r#"{"name":"main"}"#,
        &[("alpha", r#"{"format": 1, "schema": "default@1.0.0"}"#)],
    );
    let _ = migrate_to_memstead_ref(&gitdir).unwrap();
    match read_mem_config_from_memstead_ref(&gitdir, "nonexistent") {
        Err(MemsteadRefError::Config { path, .. }) => {
            assert!(path.contains("nonexistent"));
        }
        other => panic!("expected Config error for missing mem, got {other:?}"),
    }
}

#[test]
fn commit_config_to_memstead_creates_ref_when_absent() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    // No __MEMSTEAD ref, no __SYSTEM ref — fresh repo. The helper
    // must create __MEMSTEAD from scratch via the MustNotExist
    // precondition.
    let ctx = CommitContext::internal();
    commit_config_to_memstead_at_gitdir(
        &gitdir,
        "alpha",
        br#"{"format": 1, "schema": "default@1.0.0"}"#,
        &ctx,
        "test commit",
    )
    .unwrap();

    let entries = list_memstead_entries(&gitdir);
    assert_eq!(entries, vec!["mems/alpha/config.json".to_string()]);

    let config = read_mem_config_from_memstead_ref(&gitdir, "alpha").unwrap();
    assert_eq!(config.schema.unwrap().to_string(), "default@1.0.0");
}

#[test]
fn commit_config_to_memstead_overwrites_existing_blob() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let ctx = CommitContext::internal();

    commit_config_to_memstead_at_gitdir(
        &gitdir,
        "alpha",
        br#"{"format": 1, "schema": "default@1.0.0"}"#,
        &ctx,
        "first",
    )
    .unwrap();
    commit_config_to_memstead_at_gitdir(
        &gitdir,
        "alpha",
        br#"{"format": 1, "schema": "default@2.0.0"}"#,
        &ctx,
        "second",
    )
    .unwrap();

    let config = read_mem_config_from_memstead_ref(&gitdir, "alpha").unwrap();
    assert_eq!(config.schema.unwrap().to_string(), "default@2.0.0");
}

#[test]
fn commit_config_to_memstead_preserves_sibling_mem_entries() {
    let tmp = TempDir::new().unwrap();
    let gitdir = fresh_repo_dir(tmp.path());
    let ctx = CommitContext::internal();

    commit_config_to_memstead_at_gitdir(
        &gitdir,
        "alpha",
        br#"{"format": 1, "schema": "default@1.0.0"}"#,
        &ctx,
        "alpha",
    )
    .unwrap();
    commit_config_to_memstead_at_gitdir(
        &gitdir,
        "beta",
        br#"{"format": 1, "schema": "default@1.0.0"}"#,
        &ctx,
        "beta",
    )
    .unwrap();

    let entries = list_memstead_entries(&gitdir);
    assert_eq!(
        entries,
        vec![
            "mems/alpha/config.json".to_string(),
            "mems/beta/config.json".to_string(),
        ]
    );
}

#[test]
fn extract_manifest_version_handles_quoted_and_unquoted() {
    assert_eq!(
        extract_manifest_version("name: foo\nversion: \"1.0.0\"\n"),
        Some("1.0.0".to_string())
    );
    assert_eq!(
        extract_manifest_version("name: foo\nversion: 1.0.0\n"),
        Some("1.0.0".to_string())
    );
    assert_eq!(
        extract_manifest_version("name: foo\nversion: '0.5.0'\n"),
        Some("0.5.0".to_string())
    );
    assert_eq!(extract_manifest_version("name: foo\n"), None);
    assert_eq!(extract_manifest_version(""), None);
}
