#![cfg(test)]

use super::*;
use std::path::Path;
use tempfile::TempDir;

use crate::backend::{BackendError, MemBackend};
use crate::engine::test_helpers::{cli_actor, empty_create_args, folder_mount};
use crate::storage::FilesystemBackend;
use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

/// Seed a folder-backed mem with `.memstead/config.json` and N
/// entities (zero allowed); return the running engine + mem dir.
fn folder_mem_with_entities(tmp: &TempDir, titles: &[&str]) -> (Engine, std::path::PathBuf) {
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    let config_body = r#"{
            "format": 1,
            "schema": "default@1.0.0",
            "version": "1.0.0"
        }"#;
    std::fs::write(mem_dir.join(".memstead").join("config.json"), config_body).unwrap();

    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    for t in titles {
        engine
            .create_entity(empty_create_args("specs", t), actor, Some(&client), None)
            .unwrap();
    }
    (engine, mem_dir)
}

#[test]
fn export_to_bytes_produces_bytes_that_extract_cleanly() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha", "Beta"]);
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    assert!(!bytes.is_empty(), "export bytes must be non-empty");
    // The bytes validate against the archive ingress validator
    // standalone — any consumer of sealed archives accepts them.
    let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert_eq!(entries.markdown_files.len(), 2);
    let mut names: Vec<_> = entries
        .markdown_files
        .iter()
        .map(|m| m.path.clone())
        .collect();
    names.sort();
    assert_eq!(names, vec!["alpha.md".to_string(), "beta.md".to_string()]);
}

/// Export → validate/canonicalise (the install leg) → read
/// preserves title, scope, method, and exclusions exactly —
/// ordering and non-ASCII included, empty-exclusions included —
/// and a mem without either exports as today (format aside).
#[test]
fn export_round_trips_title_and_subject() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        serde_json::json!({
            "format": 1,
            "schema": "default@1.0.0",
            "version": "1.0.0",
            "title": "Sample subject Übersicht",
            "subject": {
                "scope": "A sample scope: naïve façade, what is covered and what is not",
                "method": "Primärquellen, händisch geprüft",
                "exclusions": ["Einträge nach 2023", "Presseberichte", "Άλλα θέματα"],
            },
        })
        .to_string(),
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    engine
        .create_entity(
            empty_create_args("specs", "Alpha"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    // Install leg: validate + canonical repack, then read the config.
    let validated =
        crate::validator::validate_and_normalize_archive(&bytes).expect("archive re-validates");
    let cfg = &validated.config;
    assert_eq!(cfg.format, memstead_schema::PUBLISHED_MEM_FORMAT);
    assert_eq!(cfg.title.as_deref(), Some("Sample subject Übersicht"));
    let subject = cfg.subject.as_ref().expect("subject rides the archive");
    assert_eq!(
        subject.scope,
        "A sample scope: naïve façade, what is covered and what is not"
    );
    assert_eq!(
        subject.method.as_deref(),
        Some("Primärquellen, händisch geprüft")
    );
    assert_eq!(
        subject.exclusions,
        vec!["Einträge nach 2023", "Presseberichte", "Άλλα θέματα"],
        "exclusions preserved in order, non-ASCII intact"
    );

    // Empty-exclusions case round-trips as an empty list, not a drop.
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        serde_json::json!({
            "format": 1,
            "schema": "default@1.0.0",
            "version": "1.0.0",
            "subject": { "scope": "Nur der Rahmen", "exclusions": [] },
        })
        .to_string(),
    )
    .unwrap();
    engine.reload_each_writable_mem().unwrap();
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let validated = crate::validator::validate_and_normalize_archive(&bytes).expect("re-validates");
    let subject = validated.config.subject.as_ref().expect("subject present");
    assert_eq!(subject.scope, "Nur der Rahmen");
    assert_eq!(subject.method, None);
    assert!(subject.exclusions.is_empty());
    assert_eq!(validated.config.title, None, "unset title stays unset");
}

/// End-to-end producer → consumer round-trip: an entity created with
/// an authoring note exports per-entity provenance into the archive,
/// and a fresh engine that installs those bytes reads the rationale
/// back — matching the source. An entity created without a note is
/// absent from the payload (no fabricated provenance).
#[test]
fn export_carries_provenance_that_install_reads_back() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    // Alpha carries a note; Beta deliberately does not.
    engine
        .create_entity(
            empty_create_args("specs", "Alpha"),
            actor,
            Some(&client),
            Some("why alpha exists"),
        )
        .unwrap();
    engine
        .create_entity(
            empty_create_args("specs", "Beta"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    // The archive carries the provenance payload.
    let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert!(
        entries.provenance_bytes.is_some(),
        "export must embed the provenance payload"
    );

    // The publish/install store path persists the *canonical* (re-packed)
    // bytes, not the raw upload — so normalize must preserve the
    // provenance member or it would be dropped before serving.
    let validated =
        crate::validator::validate_and_normalize_archive(&bytes).expect("archive re-validates");
    let canonical_entries =
        extract_entries(&validated.canonical_bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert!(
        canonical_entries.provenance_bytes.is_some(),
        "normalize must preserve provenance through the canonical re-pack (publish store path)"
    );

    // Install the bytes into a fresh engine and read provenance back.
    let installed = Engine::from_archive_bytes(bytes).unwrap();
    let prov = installed
        .archive_provenance_for("specs")
        .expect("installed mem exposes provenance");
    assert_eq!(
        prov.entity("alpha").and_then(|r| r.rationale.as_deref()),
        Some("why alpha exists"),
        "noted entity's rationale matches the source"
    );
    assert_eq!(
        prov.entity("alpha").and_then(|r| r.kind.as_deref()),
        Some("create"),
    );
    let beta = prov
        .entity("beta")
        .expect("every carried entity has a record, noted or not");
    assert!(
        beta.rationale.is_none() && beta.kind.is_none(),
        "entity authored without a note carries an explicit no-rationale record — nothing fabricated"
    );
}

/// Inject an extra member into a zip archive, returning fresh bytes.
/// Export now embeds anchors natively (see
/// [`export_embeds_anchors_that_install_reads_back`]); this helper still
/// synthesises the member in isolation so the canonical-repack survival
/// test exercises the registry path independent of the export producer.
fn inject_zip_member(archive: &[u8], name: &str, content: &[u8]) -> Vec<u8> {
    use std::io::{Read, Write};
    let mut src = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
    let mut out = Vec::new();
    {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for i in 0..src.len() {
            let mut f = src.by_index(i).unwrap();
            let fname = f.name().to_string();
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
            w.start_file(fname, opts).unwrap();
            w.write_all(&buf).unwrap();
        }
        w.start_file(name, opts).unwrap();
        w.write_all(content).unwrap();
        w.finish().unwrap();
    }
    out
}

/// Replace one member's bytes in a zip archive, returning fresh
/// bytes — the tamper helper for negative format tests.
fn rewrite_zip_member(archive: &[u8], name: &str, content: &[u8]) -> Vec<u8> {
    use std::io::{Read, Write};
    let mut src = zip::ZipArchive::new(std::io::Cursor::new(archive)).unwrap();
    let mut out = Vec::new();
    {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for i in 0..src.len() {
            let mut f = src.by_index(i).unwrap();
            let fname = f.name().to_string();
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
            w.start_file(fname.clone(), opts).unwrap();
            if fname == name {
                w.write_all(content).unwrap();
            } else {
                w.write_all(&buf).unwrap();
            }
        }
        w.finish().unwrap();
    }
    out
}

/// The format gate has no reader path that bypasses it: an archive rewritten to `format: 99` used to
/// hydrate through the byte path (and hence the wasm package) and
/// serve every entity. It now refuses typed; the untampered archive
/// keeps hydrating.
#[test]
fn byte_hydration_refuses_unknown_archive_format() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, _dir) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let _ = &mut engine;
    let exported = engine.export_mem_to_bytes("specs").unwrap();

    // Complement first: the untampered archive hydrates.
    let ok = Engine::from_archive_bytes(exported.clone()).expect("valid archive hydrates");
    assert!(
        ok.get_entity(&crate::EntityId("specs--alpha".into()))
            .is_some()
    );

    // Tamper: rewrite the declared format to an unknown value.
    let entries = extract_entries(&exported, &ValidatorLimits::DEFAULT).unwrap();
    let mut cfg: serde_json::Value = serde_json::from_slice(&entries.config_bytes).unwrap();
    cfg["format"] = serde_json::json!(99);
    let tampered = rewrite_zip_member(
        &exported,
        ".memstead/config.json",
        serde_json::to_string(&cfg).unwrap().as_bytes(),
    );

    let err =
        Engine::from_archive_bytes(tampered).expect_err("format 99 must refuse, never hydrate");
    match err {
        FromArchiveBytesError::UnsupportedFormat { declared, accepted } => {
            assert_eq!(declared, 99);
            assert!(!accepted.is_empty());
        }
        other => panic!("expected UnsupportedFormat, got {other:?}"),
    }
}

/// Registry-leg survival: an anchors sidecar member threads verbatim
/// through `validate_and_normalize_archive`'s canonical re-pack (the
/// publish/install store path) rather than being silently stripped, and
/// the installed mem exposes the anchors.
#[test]
fn anchors_member_survives_canonical_repack_and_install() {
    let tmp = TempDir::new().unwrap();
    let (mut engine, _dir) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let _ = &mut engine;
    let exported = engine.export_mem_to_bytes("specs").unwrap();

    let anchors = br#"{"version":1,"entities":{"specs--alpha":[{"artifact":"src/lib.rs","grain":"file","class":"anchored","hash_stability":"stable","hash":"h1"}]}}"#;
    let with_anchors = inject_zip_member(&exported, ".memstead/anchors.json", anchors);

    // Recognised at extract time.
    let entries = extract_entries(&with_anchors, &ValidatorLimits::DEFAULT).unwrap();
    assert_eq!(entries.anchors_bytes.as_deref(), Some(&anchors[..]));

    // Threaded through the canonical re-pack (what publish stores).
    let validated = crate::validator::validate_and_normalize_archive(&with_anchors)
        .expect("archive with anchors re-validates");
    let canonical = extract_entries(&validated.canonical_bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert_eq!(
        canonical.anchors_bytes.as_deref(),
        Some(&anchors[..]),
        "normalize must preserve the anchors member through the canonical re-pack"
    );

    // Installing the canonical bytes exposes the anchors on the mem.
    let installed = Engine::from_archive_bytes(validated.canonical_bytes).unwrap();
    let ids = installed.entity_anchors(&crate::EntityId::new("specs", "alpha"));
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].artifact, "src/lib.rs");
}

/// End-to-end export leg: an entity created with an
/// `anchors[]` payload exports the anchors sidecar *natively* inside the
/// `.mem` archive (no injection), the canonical re-pack preserves it, and
/// a fresh engine that installs the bytes reads the anchor back — matching
/// the source. A mem with no anchors embeds no member.
#[test]
fn export_embeds_anchors_that_install_reads_back() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    // Alpha carries a file anchor; Beta carries none.
    let mut alpha = empty_create_args("specs", "Alpha");
    alpha.anchors = vec![crate::anchor::AnchorInput {
        artifact: Some("src/lib.rs".to_string()),
        grain: Some("file".to_string()),
        class: Some("anchored".to_string()),
        hash: Some("h1".to_string()),
        hash_stability: Some("stable".to_string()),
        ..Default::default()
    }];
    engine
        .create_entity(alpha, actor, Some(&client), None)
        .unwrap();
    engine
        .create_entity(
            empty_create_args("specs", "Beta"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    // Export embeds the anchors sidecar natively (producer half of the
    // recognised-member contract).
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert!(
        entries.anchors_bytes.is_some(),
        "export must embed the anchors sidecar when the mem has anchors"
    );

    // Canonical re-pack (publish store path) preserves it.
    let validated =
        crate::validator::validate_and_normalize_archive(&bytes).expect("archive re-validates");
    let canonical = extract_entries(&validated.canonical_bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert!(
        canonical.anchors_bytes.is_some(),
        "normalize must preserve the exported anchors member"
    );

    // Install into a fresh engine and read the anchor back.
    let installed = Engine::from_archive_bytes(validated.canonical_bytes).unwrap();
    let alpha_anchors = installed.entity_anchors(&crate::EntityId::new("specs", "alpha"));
    assert_eq!(alpha_anchors.len(), 1);
    assert_eq!(alpha_anchors[0].artifact, "src/lib.rs");
    assert_eq!(alpha_anchors[0].hash.as_deref(), Some("h1"));
    // Beta had no anchors — none fabricated.
    assert!(
        installed
            .entity_anchors(&crate::EntityId::new("specs", "beta"))
            .is_empty(),
        "an entity with no anchors exposes none after install"
    );
}

/// Publish-time redaction (W6/03): redacting an exported archive's
/// anchors passes validation, survives the canonical re-pack, and
/// installs with the redacted sidecar reading back intact — the
/// sentinel where the artifact reference was, the trust metadata
/// (class, hash) untouched, and the anchor count unchanged. The
/// unredacted bytes are not altered by the transform's existence:
/// an archive without the flag stays byte-identical, and a redacted
/// archive of a mem with NO anchors is byte-identical input too.
#[test]
fn redacted_archive_survives_repack_and_install() {
    use crate::filesystem::publish::redact_archive_anchors;

    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let mut alpha = empty_create_args("specs", "Alpha");
    alpha.anchors = vec![crate::anchor::AnchorInput {
        artifact: Some("src/secret/module.rs".to_string()),
        grain: Some("file".to_string()),
        class: Some("anchored".to_string()),
        hash: Some("h1".to_string()),
        hash_stability: Some("stable".to_string()),
        ..Default::default()
    }];
    engine
        .create_entity(alpha, actor, Some(&client), None)
        .unwrap();

    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let redacted = redact_archive_anchors(&bytes).unwrap();
    assert_ne!(redacted, bytes, "redaction changes the anchors member");

    // Validation and the canonical re-pack accept the redacted package.
    let validated = crate::validator::validate_and_normalize_archive(&redacted)
        .expect("redacted archive validates");
    let canonical = extract_entries(&validated.canonical_bytes, &ValidatorLimits::DEFAULT).unwrap();
    let sidecar = crate::anchor::AnchorSidecar::from_bytes(
        canonical.anchors_bytes.as_deref().expect("member survives"),
    )
    .unwrap();
    let anchors = sidecar.get("specs--alpha");
    assert_eq!(anchors.len(), 1, "anchor count unchanged");
    assert_eq!(
        anchors[0].artifact,
        crate::anchor::REDACTED_ARTIFACT_SENTINEL
    );
    assert_eq!(
        anchors[0].hash.as_deref(),
        Some("h1"),
        "trust metadata kept"
    );

    // Install reads the redacted state back honestly.
    let installed = Engine::from_archive_bytes(validated.canonical_bytes).unwrap();
    let read_back = installed.entity_anchors(&crate::EntityId::new("specs", "alpha"));
    assert_eq!(read_back.len(), 1);
    assert_eq!(
        read_back[0].artifact,
        crate::anchor::REDACTED_ARTIFACT_SENTINEL
    );
    assert!(
        !String::from_utf8_lossy(&redacted).contains("src/secret/module.rs"),
        "the artifact path must not survive anywhere in the package"
    );

    // The workspace the author published from is unchanged: the local
    // sidecar still carries the real reference, and local anchors
    // rendering reads it — redaction happened on the staged copy only.
    let local = std::fs::read_to_string(mem_dir.join(".memstead").join("anchors.json")).unwrap();
    assert!(
        local.contains("src/secret/module.rs"),
        "local sidecar bytes untouched by a redacted publish"
    );
    assert_eq!(
        engine.entity_anchors(&crate::EntityId::new("specs", "alpha"))[0].artifact,
        "src/secret/module.rs"
    );

    // A mem with no anchors: the transform is a byte-identical no-op.
    let mem_dir2 = tmp.path().join("plain");
    std::fs::create_dir_all(mem_dir2.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir2.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let writer2 = FilesystemBackend::new(mem_dir2.clone());
    let mut engine2 = Engine::from_mounts(vec![(
        folder_mount("plain", mem_dir2.clone()),
        Box::new(writer2) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine2
        .create_entity(
            empty_create_args("plain", "Only"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let plain_bytes = engine2.export_mem_to_bytes("plain").unwrap();
    assert_eq!(
        redact_archive_anchors(&plain_bytes).unwrap(),
        plain_bytes,
        "no anchors member ⇒ byte-identical passthrough"
    );
}

/// Serve sketch-session leg: an anchored write into an
/// in-memory mem round-trips through session export → re-import. The
/// in-memory backend is exactly what serve mounts, so this proves the
/// serve session-export path carries anchors without a serve dependency.
#[test]
fn in_memory_mem_export_round_trips_anchors() {
    use crate::storage::InMemoryBackend;
    // A session-style in-memory mem is self-describing: a versioned config
    // is written to the backend before boot so export can project it.
    let backend = InMemoryBackend::new();
    backend
        .write_mem_config(
            br#"{"version":"0.1.0","schema":"default@1.0.0"}"#,
            &crate::vcs::CommitContext::new(
                Some("test"),
                crate::vcs::Actor::Cli,
                None,
                None,
                crate::vcs::Role::Unspecified,
                None,
            ),
        )
        .unwrap();
    let mount = Mount {
        mem: "sketch".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::InMemory,
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    let mut engine =
        Engine::from_mounts(vec![(mount, Box::new(backend) as Box<dyn MemBackend>)]).unwrap();
    let (actor, client) = cli_actor();

    let mut args = empty_create_args("sketch", "Idea");
    args.anchors = vec![crate::anchor::AnchorInput {
        artifact: Some("notes/idea.md".to_string()),
        grain: Some("file".to_string()),
        class: Some("informed-by".to_string()),
        ..Default::default()
    }];
    engine
        .create_entity(args, actor, Some(&client), None)
        .unwrap();

    let bytes = engine.export_mem_to_bytes("sketch").unwrap();
    let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert!(
        entries.anchors_bytes.is_some(),
        "in-memory session export must carry the anchors sidecar"
    );

    let reimported = Engine::from_archive_bytes(bytes).unwrap();
    let anchors = reimported.entity_anchors(&crate::EntityId::new("sketch", "idea"));
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].artifact, "notes/idea.md");
    assert_eq!(
        anchors[0].class,
        crate::anchor::AnchorProvenanceClass::InformedBy
    );
}

/// Size discipline: provenance scales with entity count (one current
/// rationale per entity, each ≤ the 280-char note cap), so for a
/// representative mem (~60 noted entities, larger than the live engine
/// seed's ~120 but with realistic notes) the provenance-bearing archive
/// stays well under the registry's 2 MB publish body limit, and the
/// provenance payload is a small fraction of the archive.
#[test]
fn provenance_bearing_archive_stays_within_publish_budget() {
    const PUBLISH_BODY_LIMIT: usize = 2 * 1024 * 1024;
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    // A realistic-length authoring note on every entity (near the
    // 280-char cap) — the worst case for provenance size.
    let note = "x".repeat(280);
    for i in 0..60 {
        engine
            .create_entity(
                empty_create_args("specs", &format!("Entity {i}")),
                actor,
                Some(&client),
                Some(&note),
            )
            .unwrap();
    }
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    assert!(
        bytes.len() < PUBLISH_BODY_LIMIT,
        "archive ({} B) must stay under the 2 MB publish limit",
        bytes.len()
    );
    let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
    let prov = entries.provenance_bytes.expect("provenance present");
    // Every entity's rationale travelled and the payload is a modest
    // fraction of the archive, not a budget threat.
    assert!(
        prov.len() < PUBLISH_BODY_LIMIT / 4,
        "provenance payload ({} B) is a small fraction of the budget",
        prov.len()
    );
    let parsed = memstead_schema::ArchiveProvenance::from_archive_bytes(&prov).unwrap();
    assert_eq!(
        parsed.entities.len(),
        60,
        "every noted entity has provenance"
    );
}

/// A mem slice carrying a
/// cross-mem edge (target lives in another mem, won't travel in
/// this single-mem archive) exports successfully — the archive is
/// still produced — and the export surfaces the dangling edge so the
/// operator sees, before sharing, exactly what `install` will reject.
/// AC1 (export warns, archive produced) + AC2 (export's condition ==
/// install's refusal) tested against one set of bytes.
#[test]
fn export_warns_on_cross_mem_edge_that_install_refuses() {
    let tmp = TempDir::new().unwrap();
    let (engine, mem_dir) = folder_mem_with_entities(&tmp, &[]);
    // Hand-write a valid spec whose only blemish is a cross-mem
    // USES edge into mem `other` — the folder export reads `.md`
    // verbatim, so the edge lands in the archive.
    let md = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-01-15
level: M0
---
# Broker

## Identity

A

## Purpose

B

## Specifies

C

## Constraints

D

## Rationale

E

## Relationships

- **USES**: [[other--thing]]
";
    std::fs::write(mem_dir.join("broker.md"), md).unwrap();

    // The path-shaped export carries the dangling edge on its result
    // and still writes the archive (AC1).
    let out = tmp.path().join("specs.mem");
    let result = engine.export_mem("specs", &out).unwrap();
    assert!(out.is_file(), "archive must still be produced");
    assert_eq!(
        result.dangling_cross_mem_edges.len(),
        1,
        "export must surface the cross-mem edge: {:?}",
        result.dangling_cross_mem_edges
    );
    let edge = &result.dangling_cross_mem_edges[0];
    assert_eq!(edge.entity_path, "broker.md");
    assert_eq!(edge.target_id, "other--thing");
    assert_eq!(edge.target_mem, "other");

    // AC2: the exact condition export warned on is what install
    // refuses on — the strict validator rejects these same bytes.
    let bytes = std::fs::read(&out).unwrap();
    let err = crate::validator::validate_and_normalize_archive(&bytes).unwrap_err();
    assert!(
        matches!(
            err,
            crate::validator::ValidationError::CrossMemRelationship { .. }
        ),
        "install-side strict validation must refuse the same edge: {err:?}",
    );
}

/// `make_archive_self_contained` turns the archive `install` refuses
/// into one it accepts: the cross-mem row is dropped and reported,
/// the same-mem row and the body text survive, the result passes
/// strict validation and hydrates. Complement: an already
/// self-contained archive comes back with nothing dropped.
#[test]
fn self_contained_repack_drops_cross_mem_rows_and_passes_install_validation() {
    let tmp = TempDir::new().unwrap();
    let (engine, mem_dir) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let md = "\
---
type: spec
created_date: 2026-01-15
last_modified: 2026-01-15
level: M0
---
# Broker

## Identity

Talks to [[other:thing]] over the wire.

## Purpose

B

## Specifies

C

## Constraints

D

## Rationale

E

## Relationships

- **USES**: [[other--thing]]
- **REFERENCES**: [[alpha]]
";
    std::fs::write(mem_dir.join("broker.md"), md).unwrap();
    let out = tmp.path().join("specs.mem");
    engine.export_mem("specs", &out).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    assert!(
        crate::validator::validate_and_normalize_archive(&bytes).is_err(),
        "precondition: the raw export is refused"
    );

    let sealed = crate::validator::make_archive_self_contained(&bytes).unwrap();
    assert_eq!(sealed.dropped.len(), 1, "{:?}", sealed.dropped);
    assert_eq!(sealed.dropped[0].entity_path, "broker.md");
    assert_eq!(sealed.dropped[0].target_id, "other--thing");
    assert_eq!(sealed.dropped[0].target_mem, "other");
    // The proof is the strict validator, the one `install` runs.
    let validated = crate::validator::validate_and_normalize_archive(&sealed.bytes)
        .expect("the self-contained archive passes strict validation");
    assert!(validated.dangling_cross_mem_edges.is_empty());
    let broker = validated
        .entities
        .iter()
        .find(|e| e.id.as_ref() == "specs--broker")
        .expect("broker travels");
    assert_eq!(
        broker
            .relationships
            .iter()
            .map(|r| r.target.as_ref())
            .collect::<Vec<_>>(),
        vec!["specs--alpha"],
        "only the same-mem row survives"
    );
    assert!(
        broker
            .sections
            .values()
            .any(|b| b.contains("[[other:thing]]")),
        "body wiki-links are never touched"
    );
    let hydrated = Engine::from_archive_bytes(sealed.bytes).unwrap();
    assert_eq!(
        hydrated.store().all_entities().filter(|e| !e.stub).count(),
        2
    );

    // Complement: nothing to drop on an archive that is already clean.
    std::fs::remove_file(mem_dir.join("broker.md")).unwrap();
    let clean_engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir.clone()),
        Box::new(FilesystemBackend::new(mem_dir.clone())) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let clean_out = tmp.path().join("clean.mem");
    clean_engine.export_mem("specs", &clean_out).unwrap();
    let clean =
        crate::validator::make_archive_self_contained(&std::fs::read(&clean_out).unwrap()).unwrap();
    assert!(clean.dropped.is_empty());
    assert!(crate::validator::validate_and_normalize_archive(&clean.bytes).is_ok());
}

/// Complement: a self-contained export (no cross-mem edges) carries
/// no dangling-edge warnings.
#[test]
fn export_self_contained_mem_warns_nothing() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha", "Beta"]);
    let out = tmp.path().join("specs.mem");
    let result = engine.export_mem("specs", &out).unwrap();
    assert!(
        result.dangling_cross_mem_edges.is_empty(),
        "self-contained export must warn nothing: {:?}",
        result.dangling_cross_mem_edges
    );
}

#[test]
fn export_empty_mem_produces_valid_hydratable_archive() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &[]);
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    // Validator accepts the empty case.
    let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert!(entries.markdown_files.is_empty());
    // Hydrate path accepts it too.
    let hydrated = Engine::from_archive_bytes(bytes).unwrap();
    assert_eq!(hydrated.mem_names(), vec!["specs"]);
    assert!(hydrated.store().is_empty());
}

#[test]
fn export_unknown_mem_returns_unknown_mem_error() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &[]);
    let err = engine.export_mem_to_bytes("missing").unwrap_err();
    match err {
        EngineError::UnknownMem(v) => assert_eq!(v, "missing"),
        other => panic!("expected UnknownMem, got {other:?}"),
    }
}

#[test]
fn export_archive_backend_returns_sealed() {
    // Seed by exporting a folder mem, then re-mount the produced
    // archive as a read-only archive. The byte-export path on the
    // archive mount refuses with the Sealed envelope — matches the
    // existing path-based `export_mem` posture.
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let bytes = engine.export_mem_to_bytes("specs").unwrap();

    let archive_path = tmp.path().join("ext.mem");
    std::fs::write(&archive_path, &bytes).unwrap();
    let archive_engine = Engine::from_mounts(vec![(
        Mount {
            mem: "ext".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: MountStorage::Archive {
                path: archive_path.clone(),
            },
            capability: MountCapability::ReadOnly,
            lifecycle: MountLifecycle::Lazy,
            cross_linkable: false,
            migration_target: None,
        },
        Box::new(crate::storage::ArchiveBackend::new(archive_path)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let err = archive_engine.export_mem_to_bytes("ext").unwrap_err();
    assert!(matches!(err, EngineError::Backend(BackendError::Sealed)));
}

#[test]
fn from_archive_bytes_refuses_non_zip_with_validation_error() {
    let err = Engine::from_archive_bytes(b"not a zip at all".to_vec()).unwrap_err();
    match err {
        FromArchiveBytesError::Validation(crate::validator::ValidationError::Zip(_)) => {}
        other => panic!("expected Validation(Zip(_)), got {other:?}"),
    }
}

#[test]
fn from_archive_bytes_refuses_oversized_with_size_cap() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let bytes = engine.export_mem_to_bytes("specs").unwrap();

    let mut limits = ValidatorLimits::DEFAULT;
    limits.max_compressed_archive = 1;
    let err = Engine::from_archive_bytes_with_limits(bytes, &limits).unwrap_err();
    match err {
        FromArchiveBytesError::Validation(crate::validator::ValidationError::SizeCapExceeded {
            ..
        }) => {}
        other => panic!("expected Validation(SizeCapExceeded), got {other:?}"),
    }
}

#[test]
fn hydrated_engine_answers_reads_and_refuses_writes() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["Hello", "World"]);
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let mut hydrated = Engine::from_archive_bytes(bytes).unwrap();

    // Read surface — same titles surface from the hydrated state.
    let hello = hydrated
        .get_entity(&crate::EntityId::new("specs", "hello"))
        .expect("hello entity must round-trip");
    assert_eq!(hello.title, "Hello");
    let world = hydrated
        .get_entity(&crate::EntityId::new("specs", "world"))
        .expect("world entity must round-trip");
    assert_eq!(world.title, "World");

    // Mutation surface — read-only mount refuses with the existing
    // typed envelope (no new error categories on the hydrate path).
    let (actor, client) = cli_actor();
    let err = hydrated
        .create_entity(
            empty_create_args("specs", "Forbidden"),
            actor,
            Some(&client),
            None,
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::ReadOnlyMount(v) if v == "specs"));
}

#[test]
fn round_trip_preserves_entities_and_relations() {
    // Build a multi-entity mem with a relation, export → hydrate,
    // and confirm state equivalence: same ids, same content per
    // entity, same relations.
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut source = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let src = source
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let tgt = source
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    source
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: src.id.clone(),
                expected_hash: Some(src.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: tgt.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();

    let bytes = source.export_mem_to_bytes("specs").unwrap();
    let hydrated = Engine::from_archive_bytes(bytes).unwrap();

    // Same id set.
    let mut src_ids: Vec<String> = source
        .store()
        .all_entities()
        .map(|e| e.id.to_string())
        .collect();
    let mut hyd_ids: Vec<String> = hydrated
        .store()
        .all_entities()
        .map(|e| e.id.to_string())
        .collect();
    src_ids.sort();
    hyd_ids.sort();
    assert_eq!(src_ids, hyd_ids);

    // Same title + entity_type per id.
    for id_str in &src_ids {
        let (mem, slug) = id_str.split_once("--").expect("ids carry `<mem>--<slug>`");
        let id = crate::EntityId::new(mem, slug);
        let s = source.get_entity(&id).unwrap();
        let h = hydrated.get_entity(&id).unwrap();
        assert_eq!(s.title, h.title, "title differs for {id_str}");
        assert_eq!(s.entity_type, h.entity_type, "type differs for {id_str}");
    }

    // Same outgoing relation set.
    let src_edges: Vec<_> = source
        .store()
        .outgoing(&src.id)
        .iter()
        .map(|e| (e.rel_type.clone(), e.target.clone()))
        .collect();
    let hyd_edges: Vec<_> = hydrated
        .store()
        .outgoing(&src.id)
        .iter()
        .map(|e| (e.rel_type.clone(), e.target.clone()))
        .collect();
    assert_eq!(src_edges, hyd_edges);
}

#[test]
fn export_then_hydrate_then_re_export_yields_byte_equivalent_archive() {
    // Determinism check — same source state must produce identical
    // archive bytes through the export path, and re-exporting from
    // the hydrated copy is not part of the contract (the hydrated
    // engine is read-only) but the produced bytes from the source
    // must be a fixpoint when re-fed.
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let bytes1 = engine.export_mem_to_bytes("specs").unwrap();
    let bytes2 = engine.export_mem_to_bytes("specs").unwrap();
    assert_eq!(bytes1, bytes2, "export bytes must be deterministic");
}

/// Compare two engines' state for the named mem. State
/// equivalence at minimum (per the round-trip AC): same entity
/// ids, same content per entity (title, type, metadata, sections,
/// content_hash), same relations (rel_type + target per source).
/// Shared by the fixture-sweep round-trip tests so they assert the
/// same invariant regardless of the fixture shape under test.
fn assert_state_equivalent(source: &Engine, hydrated: &Engine, mem: &str) {
    let mut src_ids: Vec<String> = source
        .store()
        .all_entities()
        .filter(|e| e.mem == mem)
        .map(|e| e.id.to_string())
        .collect();
    let mut hyd_ids: Vec<String> = hydrated
        .store()
        .all_entities()
        .filter(|e| e.mem == mem)
        .map(|e| e.id.to_string())
        .collect();
    src_ids.sort();
    hyd_ids.sort();
    assert_eq!(src_ids, hyd_ids, "entity id set differs for mem {mem}");

    for id_str in &src_ids {
        let (v, slug) = id_str.split_once("--").expect("ids carry `<mem>--<slug>`");
        let id = crate::EntityId::new(v, slug);
        let s = source.get_entity(&id).expect("source entity present");
        let h = hydrated.get_entity(&id).expect("hydrated entity present");
        assert_eq!(s.title, h.title, "title differs for {id_str}");
        assert_eq!(s.entity_type, h.entity_type, "type differs for {id_str}");
        assert_eq!(s.metadata, h.metadata, "metadata differs for {id_str}");
        assert_eq!(s.sections, h.sections, "sections differ for {id_str}");
        assert_eq!(
            s.content_hash, h.content_hash,
            "content_hash differs for {id_str}",
        );

        let mut src_edges: Vec<_> = source
            .store()
            .outgoing(&id)
            .iter()
            .map(|e| (e.rel_type.clone(), e.target.to_string()))
            .collect();
        let mut hyd_edges: Vec<_> = hydrated
            .store()
            .outgoing(&id)
            .iter()
            .map(|e| (e.rel_type.clone(), e.target.to_string()))
            .collect();
        src_edges.sort();
        hyd_edges.sort();
        assert_eq!(src_edges, hyd_edges, "edges differ for {id_str}");
    }
}

/// Round-trip the engine state via export → hydrate, asserting
/// state equivalence against the input. Returns the hydrated
/// engine so individual tests can drive extra reads against it.
fn round_trip(source: &Engine, mem: &str) -> Engine {
    let bytes = source.export_mem_to_bytes(mem).unwrap();
    // The bytes pass the validator standalone — same invariant the
    // bridge consumer relies on, asserted on every fixture so a
    // future export change can't silently break ingress.
    extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
    let hydrated = Engine::from_archive_bytes(bytes).unwrap();
    assert_state_equivalent(source, &hydrated, mem);
    hydrated
}

#[test]
fn fixture_sweep_round_trip_empty() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &[]);
    let hydrated = round_trip(&engine, "specs");
    assert!(hydrated.store().is_empty());
}

#[test]
fn fixture_sweep_round_trip_single_entity() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["Solo"]);
    round_trip(&engine, "specs");
}

#[test]
fn fixture_sweep_round_trip_multi_entity_no_relations() {
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["A One", "A Two", "A Three"]);
    round_trip(&engine, "specs");
}

#[test]
fn fixture_sweep_round_trip_entity_with_metadata_and_sections() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();

    let mut sections = indexmap::IndexMap::new();
    sections.insert("identity".to_string(), "A rich body.".to_string());
    sections.insert(
        "purpose".to_string(),
        "To exercise the archive round-trip.".to_string(),
    );
    sections.insert(
        "rationale".to_string(),
        "Because the spec said so.".to_string(),
    );

    let mut metadata: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
    metadata.insert("level".to_string(), "M0".to_string());

    engine
        .create_entity(
            crate::engine::CreateEntityArgs {
                anchors: Vec::new(),
                mem: "specs".to_string(),
                title: "Rich".to_string(),
                entity_type: "spec".to_string(),
                sections,
                metadata,
                relations: Vec::new(),
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    round_trip(&engine, "specs");
}

#[test]
fn fixture_sweep_round_trip_multi_entity_with_relations() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("specs");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format":1,"schema":"default@1.0.0","version":"1.0.0"}"#,
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("specs", mem_dir),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    let (actor, client) = cli_actor();
    let src = engine
        .create_entity(
            empty_create_args("specs", "Source"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let mid = engine
        .create_entity(
            empty_create_args("specs", "Middle"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let tgt = engine
        .create_entity(
            empty_create_args("specs", "Target"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // Two outgoing edges of different rel-types from the same
    // source — the round-trip must preserve both.
    engine
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: src.id.clone(),
                expected_hash: Some(src.content_hash.clone()),
                rel_type: "USES".to_string(),
                target: mid.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let src_after = engine
        .get_entity(&src.id)
        .expect("source must still resolve");
    engine
        .relate_entity(
            crate::engine::RelateEntityArgs {
                source: src.id.clone(),
                expected_hash: Some(src_after.content_hash.clone()),
                rel_type: "PART_OF".to_string(),
                target: tgt.id.clone(),
                remove: false,
                description: None,
                dry_run: false,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    round_trip(&engine, "specs");
}

#[test]
fn read_entity_path_works_against_byte_backed_archive() {
    // Sanity: the byte-backed ArchiveBackend the hydrate path
    // constructs answers `read_entity` for every listed path.
    let tmp = TempDir::new().unwrap();
    let (engine, _mem) = folder_mem_with_entities(&tmp, &["First", "Second"]);
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let hydrated = Engine::from_archive_bytes(bytes).unwrap();
    let first = hydrated
        .get_entity(&crate::EntityId::new("specs", "first"))
        .expect("first must hydrate");
    assert_eq!(first.title, "First");
    // Path-based archive_path() returns None for byte-backed
    // backends — compile-time check that the contract holds.
    let backend = ArchiveBackend::from_bytes(Vec::new());
    let _: Option<&Path> = backend.archive_path();
}

/// A hierarchical mem exports under its leaf name, and every link that
/// qualified itself with the workspace path follows: `[[planning/plan-x--
/// alpha]]` (the self-qualified form the write path accepts) and
/// `[[planning/plan-x:beta]]` read, under the leaf, as ambiguous and as
/// foreign respectively — install refused such an archive until the
/// export retargeted them. The archive must pass the strict validator
/// install runs, carry the leaf as its name, and hold the retargeted links.
#[test]
fn nested_mem_exports_under_its_leaf_with_its_links_retargeted() {
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("plan-x");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format": 1, "schema": "default@1.0.0", "version": "1.0.0"}"#,
    )
    .unwrap();
    let spec = |title: &str, identity: &str| {
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-01-15\nlevel: M0\n---\n\
# {title}\n\n## Identity\n\n{identity}\n\n## Purpose\n\nB\n\n## Specifies\n\nC\n\n\
## Constraints\n\nD\n\n## Rationale\n\nE\n"
        )
    };
    std::fs::write(mem_dir.join("alpha.md"), spec("Alpha", "A")).unwrap();
    std::fs::write(mem_dir.join("beta.md"), spec("Beta", "B")).unwrap();
    std::fs::write(
        mem_dir.join("gamma.md"),
        spec(
            "Gamma",
            "Builds on [[planning/plan-x--alpha]] and [[planning/plan-x:beta|beta]]; \
compare `[[planning/plan-x--alpha]]` in code.",
        ),
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let engine = Engine::from_mounts(vec![(
        folder_mount("planning/plan-x", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();

    let bytes = engine.export_mem_to_bytes("planning/plan-x").unwrap();
    let validated = crate::validator::validate_and_normalize_archive(&bytes)
        .expect("the strict pass install runs must accept the export");
    assert_eq!(validated.config.name, "plan-x");

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let mut gamma = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("gamma.md").unwrap(), &mut gamma).unwrap();
    assert!(gamma.contains("[[plan-x--alpha]]"), "{gamma}");
    assert!(gamma.contains("[[plan-x:beta|beta]]"), "{gamma}");
    assert!(gamma.contains("`[[planning/plan-x--alpha]]`"), "{gamma}");
    assert!(!gamma.contains("[[planning/plan-x:beta"), "{gamma}");

    // The strict pass resolves both links inside the archive: gamma's
    // outgoing body links land on alpha and beta, no cross-mem edge.
    let self_contained = crate::validator::make_archive_self_contained(&bytes).unwrap();
    assert!(
        self_contained.dropped.is_empty(),
        "{:?}",
        self_contained.dropped
    );
}

/// The defect: a hierarchical mem publishes under its leaf and the
/// export retargets its self-qualified links, so the archived bytes hash
/// differently from the bytes the sealed records were keyed to, and on
/// the mount every record read `check_stale`. Now a record fresh at
/// export is re-keyed to the archived bytes (marked `carried_from`,
/// reason `export`) and reads `checked_ok` on the mount; a record stale
/// before the export keeps its hash and still reads stale. The same holds
/// through the self-contained re-pack, which re-renders every entity.
#[test]
fn nested_mem_export_rekeys_fresh_sealed_checks_to_the_archived_bytes() {
    use crate::check::{CheckKind, CheckState, RecordKind, Verdict};
    let tmp = TempDir::new().unwrap();
    let mem_dir = tmp.path().join("planning").join("plan-x");
    std::fs::create_dir_all(mem_dir.join(".memstead")).unwrap();
    std::fs::write(
        mem_dir.join(".memstead").join("config.json"),
        r#"{"format": 1, "schema": "default@1.0.0", "version": "1.0.0"}"#,
    )
    .unwrap();
    let spec = |title: &str, identity: &str| {
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-01-15\nlevel: M0\n---\n\
# {title}\n\n## Identity\n\n{identity}\n\n## Purpose\n\nB\n\n## Specifies\n\nC\n\n\
## Constraints\n\nD\n\n## Rationale\n\nE\n"
        )
    };
    std::fs::write(mem_dir.join("alpha.md"), spec("Alpha", "A")).unwrap();
    std::fs::write(mem_dir.join("beta.md"), spec("Beta", "B")).unwrap();
    std::fs::write(
        mem_dir.join("gamma.md"),
        spec("Gamma", "Builds on [[planning/plan-x--alpha]]."),
    )
    .unwrap();
    let writer = FilesystemBackend::new(mem_dir.clone());
    let mut engine = Engine::from_mounts(vec![(
        folder_mount("planning/plan-x", mem_dir.clone()),
        Box::new(writer) as Box<dyn MemBackend>,
    )])
    .unwrap();
    engine.set_workspace_root(tmp.path().to_path_buf());
    let (actor, client) = cli_actor();
    engine.set_identity(Some("checker-s1".to_string()));
    for slug in ["gamma", "beta"] {
        engine
            .record_check_with(
                "planning/plan-x",
                &format!("planning/plan-x--{slug}"),
                Verdict::Ok,
                &RecordKind::Engine(CheckKind::Verification),
                None,
                None,
                actor,
                Some(&client),
            )
            .unwrap();
    }
    // Alpha's record is keyed to content the checker never saw: stale
    // before the export, stale after it.
    crate::check::CheckLedger::for_workspace(tmp.path())
        .record(&crate::check::CheckRecord {
            ts: 1,
            entity: "planning/plan-x--alpha".to_string(),
            verdict: "ok".to_string(),
            method: None,
            entity_hash: "stale-before-export".to_string(),
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
            identity: Some("checker-s1".to_string()),
            kind: None,
            schema_ref: None,
            finding: None,
            renamed_from: None,
            carried_from: None,
        })
        .unwrap();
    let source_gamma_hash = engine
        .get_entity(&crate::EntityId::new("planning/plan-x", "gamma"))
        .unwrap()
        .content_hash
        .clone();

    let bytes = engine.export_mem_to_bytes("planning/plan-x").unwrap();
    let sealed = sealed_checks_of(&bytes).expect("the archive carries the checks member");
    let gamma = sealed.latest("gamma", "verification").unwrap();
    assert_ne!(
        gamma.entity_hash, source_gamma_hash,
        "re-keyed to the archived bytes"
    );
    assert_eq!(
        gamma.carried_from,
        Some(crate::check::CarriedFrom::export(&source_gamma_hash))
    );
    // Beta's bytes carry no self-qualified link: untouched, unmarked.
    assert!(
        sealed
            .latest("beta", "verification")
            .unwrap()
            .carried_from
            .is_none()
    );
    assert_eq!(
        sealed.latest("alpha", "verification").unwrap().entity_hash,
        "stale-before-export"
    );

    let assert_mount = |bytes: Vec<u8>, label: &str| {
        let mounted = Engine::from_archive_bytes(bytes).unwrap();
        for slug in ["gamma", "beta"] {
            let (state, latest) = mounted
                .entity_check_state("plan-x", &format!("plan-x--{slug}"))
                .unwrap();
            assert_eq!(state, CheckState::CheckedOk, "{label}: {slug}");
            assert_eq!(
                latest.unwrap().identity.as_deref(),
                Some("checker-s1"),
                "{label}: the record stays the checker's"
            );
        }
        let (state, _) = mounted
            .entity_check_state("plan-x", "plan-x--alpha")
            .unwrap();
        assert_eq!(state, CheckState::CheckStale, "{label}: stale stays stale");
    };
    assert_mount(bytes.clone(), "export");

    // The self-contained re-pack re-renders every entity; the records
    // follow those bytes too, and the pass is idempotent.
    let self_contained = crate::validator::make_archive_self_contained(&bytes).unwrap();
    assert_mount(self_contained.bytes.clone(), "self-contained");
    let again = crate::validator::make_archive_self_contained(&self_contained.bytes).unwrap();
    assert_eq!(
        again.bytes, self_contained.bytes,
        "canonical re-pack is idempotent"
    );
}

// ---------------------------------------------------------------------------
// Sealed check records (`.memstead/checks.json`)
// ---------------------------------------------------------------------------

/// A folder mem under a workspace root (so the engine has a check
/// ledger), with `titles` created and no check recorded yet.
fn ledgered_folder_mem(tmp: &TempDir, titles: &[&str]) -> Engine {
    let (mut engine, _dir) = folder_mem_with_entities(tmp, titles);
    engine.set_workspace_root(tmp.path().to_path_buf());
    engine
}

fn check_as(
    engine: &mut Engine,
    identity: &str,
    id: &str,
    verdict: crate::check::Verdict,
    kind: crate::check::RecordKind,
    method: Option<&str>,
) {
    let (actor, client) = cli_actor();
    engine.set_identity(Some(identity.to_string()));
    engine
        .record_check_with(
            "specs",
            id,
            verdict,
            &kind,
            method,
            None,
            actor,
            Some(&client),
        )
        .unwrap();
}

fn sealed_checks_of(bytes: &[u8]) -> Option<crate::check::SealedChecks> {
    let entries = extract_entries(bytes, &ValidatorLimits::DEFAULT).unwrap();
    entries
        .checks_bytes
        .map(|b| crate::check::SealedChecks::from_archive_bytes(&b).unwrap())
}

/// AC1, folder path: the archive carries, per entity, the latest ledger
/// record per kind (verification, conformance, a foreign `x-` kind) with
/// the identity handles verbatim, the method note redacted by the same
/// classes the provenance member applies, the bytes deterministic.
/// Complement: a record for an entity the archive does not carry
/// (deleted since) and a record of another mem are not sealed, and no
/// person name reaches the archive.
#[test]
fn export_seals_latest_check_per_entity_and_kind() {
    use crate::check::{CheckKind, RecordKind, Verdict};
    let tmp = TempDir::new().unwrap();
    let mut engine = ledgered_folder_mem(&tmp, &["Alpha", "Beta", "Gamma"]);

    // Alpha: two verification checks under two handles (the later one
    // wins), a conformance check under a third, a foreign kind under a
    // fourth. The winning method names a private path.
    check_as(
        &mut engine,
        "checker-s1",
        "specs--alpha",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Verification),
        Some("first pass"),
    );
    check_as(
        &mut engine,
        "checker-s2",
        "specs--alpha",
        Verdict::Failed,
        RecordKind::Engine(CheckKind::Verification),
        // Built at runtime so the literal never matches the leak scan's
        // absolute-user-paths class (the redaction test's own discipline).
        Some(&format!(
            "diffed against {}/notes.md",
            ["/Users", "dasboe"].join("/")
        )),
    );
    check_as(
        &mut engine,
        "grader-s3",
        "specs--alpha",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Conformance),
        None,
    );
    check_as(
        &mut engine,
        "attacker-m",
        "specs--alpha",
        Verdict::Ok,
        RecordKind::Foreign("x-audit".to_string()),
        Some("adversarial read"),
    );
    // Beta: one check. Gamma: checked, then deleted.
    check_as(
        &mut engine,
        "checker-s1",
        "specs--beta",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Verification),
        None,
    );
    check_as(
        &mut engine,
        "checker-s1",
        "specs--gamma",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Verification),
        None,
    );
    let (actor, client) = cli_actor();
    engine
        .delete_entity(
            crate::DeleteEntityArgs {
                id: crate::EntityId::new("specs", "gamma"),
                expected_hash: None,
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    // A record of another mem sits in the same workspace ledger.
    crate::check::CheckLedger::for_workspace(tmp.path())
        .record(&crate::check::CheckRecord {
            ts: 1,
            entity: "other--thing".to_string(),
            verdict: "ok".to_string(),
            method: None,
            entity_hash: "h".to_string(),
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
            identity: Some("checker-elsewhere".to_string()),
            kind: None,
            schema_ref: None,
            finding: None,
            renamed_from: None,
            carried_from: None,
        })
        .unwrap();

    let report = engine.export_mem_bytes_report("specs").unwrap();
    let sealed = sealed_checks_of(&report.bytes).expect("the archive carries the checks member");
    assert_eq!(sealed.version, crate::check::SEALED_CHECKS_VERSION);
    assert_eq!(
        sealed.entities.keys().cloned().collect::<Vec<_>>(),
        vec!["alpha".to_string(), "beta".to_string()],
        "a deleted entity and another mem's entity are not sealed"
    );

    let alpha = &sealed.entities["alpha"];
    assert_eq!(
        alpha.keys().cloned().collect::<Vec<_>>(),
        vec!["conformance", "verification", "x-audit"]
    );
    let v = &alpha["verification"];
    assert_eq!(v.verdict, "failed", "the later record per kind wins");
    assert_eq!(v.identity.as_deref(), Some("checker-s2"));
    assert_eq!(v.role, "unspecified");
    assert_eq!(
        v.method.as_deref(),
        Some("diffed against [redacted:absolute-user-paths]/notes.md"),
        "the method note passes the provenance member's redaction"
    );
    let c = &alpha["conformance"];
    assert_eq!(c.identity.as_deref(), Some("grader-s3"));
    assert_eq!(c.schema_ref.as_deref(), Some("default@1.0.0"));
    let x = &alpha["x-audit"];
    assert_eq!(x.identity.as_deref(), Some("attacker-m"));
    assert_eq!(x.method.as_deref(), Some("adversarial read"));
    assert_eq!(sealed.entities["beta"].len(), 1);

    // The sealed hash is the entity's content hash at check time.
    let alpha_hash = engine
        .get_entity(&crate::EntityId::new("specs", "alpha"))
        .unwrap()
        .content_hash
        .clone();
    assert_eq!(v.entity_hash, alpha_hash);

    // Nothing that is not on the ledger line, and no person name.
    let raw = String::from_utf8(
        extract_entries(&report.bytes, &ValidatorLimits::DEFAULT)
            .unwrap()
            .checks_bytes
            .unwrap(),
    )
    .unwrap();
    assert!(!raw.contains("dasboe"), "{raw}");
    assert!(!raw.contains("\"client\""), "{raw}");
    assert!(!raw.contains("\"finding\""), "{raw}");
    assert!(!raw.contains("checker-elsewhere"), "{raw}");

    // The export report counts the redaction, by class.
    assert_eq!(report.redactions.len(), 1, "{:?}", report.redactions);
    assert_eq!(report.redactions[0].class, "absolute-user-paths");
    assert_eq!(report.redactions[0].count, 1);

    // Deterministic for a given ledger.
    let again = engine.export_mem_to_bytes("specs").unwrap();
    assert_eq!(report.bytes, again);
}

/// AC1 complement: an engine with no workspace root, a workspace with no
/// ledger, and a mem with no record in the ledger all export the same
/// bytes with no member.
#[test]
fn export_without_a_qualifying_record_embeds_no_checks_member() {
    let tmp = TempDir::new().unwrap();
    let (engine, _dir) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let rootless = engine.export_mem_to_bytes("specs").unwrap();
    assert!(sealed_checks_of(&rootless).is_none());

    let mut engine = engine;
    engine.set_workspace_root(tmp.path().to_path_buf());
    let no_ledger = engine.export_mem_to_bytes("specs").unwrap();
    assert_eq!(rootless, no_ledger, "no ledger: byte-identical, no member");

    // A ledger that holds only another mem's record.
    crate::check::CheckLedger::for_workspace(tmp.path())
        .record(&crate::check::CheckRecord {
            ts: 1,
            entity: "other--thing".to_string(),
            verdict: "ok".to_string(),
            method: None,
            entity_hash: "h".to_string(),
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
            identity: None,
            kind: None,
            schema_ref: None,
            finding: None,
            renamed_from: None,
            carried_from: None,
        })
        .unwrap();
    let no_record = engine.export_mem_to_bytes("specs").unwrap();
    assert_eq!(rootless, no_record, "no record for this mem: no member");
}

/// AC1, in-memory path: a session mem under a workspace root seals its
/// check records through the same builder.
#[test]
fn in_memory_export_seals_check_records() {
    use crate::check::{CheckKind, RecordKind, Verdict};
    use crate::storage::InMemoryBackend;
    let tmp = TempDir::new().unwrap();
    let backend = InMemoryBackend::new();
    backend
        .write_mem_config(
            br#"{"version":"0.1.0","schema":"default@1.0.0"}"#,
            &crate::vcs::CommitContext::new(
                Some("test"),
                crate::vcs::Actor::Cli,
                None,
                None,
                crate::vcs::Role::Unspecified,
                None,
            ),
        )
        .unwrap();
    let mount = Mount {
        mem: "specs".to_string(),
        schema: Some("default@1.0.0".parse().unwrap()),
        storage: MountStorage::InMemory,
        capability: MountCapability::Write,
        lifecycle: MountLifecycle::Eager,
        cross_linkable: false,
        migration_target: None,
    };
    let mut engine =
        Engine::from_mounts(vec![(mount, Box::new(backend) as Box<dyn MemBackend>)]).unwrap();
    engine.set_workspace_root(tmp.path().to_path_buf());
    let (actor, client) = cli_actor();
    engine
        .create_entity(
            empty_create_args("specs", "Idea"),
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    check_as(
        &mut engine,
        "checker-s9",
        "specs--idea",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Verification),
        None,
    );
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let sealed = sealed_checks_of(&bytes).expect("in-memory export seals the checks member");
    assert_eq!(
        sealed
            .latest("idea", "verification")
            .and_then(|r| r.identity.as_deref()),
        Some("checker-s9")
    );
}

/// AC2: the member threads verbatim through the canonical re-pack (what
/// install stores) and the installed mem exposes it; an archive without
/// the member installs as today and exposes none.
#[test]
fn checks_member_survives_canonical_repack_and_install() {
    use crate::check::{CheckKind, RecordKind, Verdict};
    let tmp = TempDir::new().unwrap();
    let mut engine = ledgered_folder_mem(&tmp, &["Alpha"]);
    check_as(
        &mut engine,
        "checker-s1",
        "specs--alpha",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Verification),
        None,
    );
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let raw = extract_entries(&bytes, &ValidatorLimits::DEFAULT)
        .unwrap()
        .checks_bytes
        .expect("member present");

    let validated =
        crate::validator::validate_and_normalize_archive(&bytes).expect("archive re-validates");
    assert_eq!(validated.checks_bytes.as_deref(), Some(&raw[..]));
    let canonical = extract_entries(&validated.canonical_bytes, &ValidatorLimits::DEFAULT).unwrap();
    assert_eq!(
        canonical.checks_bytes.as_deref(),
        Some(&raw[..]),
        "normalize must preserve the checks member through the canonical re-pack"
    );

    let installed = Engine::from_archive_bytes(validated.canonical_bytes).unwrap();
    let sealed = installed
        .archive_checks_for("specs")
        .expect("installed mem exposes its sealed checks");
    assert!(sealed.latest("alpha", "verification").is_some());

    // Without the member: as today.
    let plain_tmp = TempDir::new().unwrap();
    let (plain, _dir) = folder_mem_with_entities(&plain_tmp, &["Alpha"]);
    let plain_bytes = plain.export_mem_to_bytes("specs").unwrap();
    let validated = crate::validator::validate_and_normalize_archive(&plain_bytes).unwrap();
    assert!(validated.checks_bytes.is_none());
    let installed = Engine::from_archive_bytes(validated.canonical_bytes).unwrap();
    assert!(installed.archive_checks_for("specs").is_none());
}

/// AC3: an archive mount derives `entity --provenance`'s check states
/// and the health checks axis from the sealed member with the workspace
/// derivation (sealed hash against the sealed entity, conformance also
/// against the pin); foreign kinds are listed; the independence reading
/// stays unconfirmable on an archive.
#[test]
fn archive_mount_derives_check_state_from_sealed_member() {
    use crate::check::{CheckKind, RecordKind, Verdict};
    let tmp = TempDir::new().unwrap();
    let mut engine = ledgered_folder_mem(&tmp, &["Alpha", "Beta"]);
    check_as(
        &mut engine,
        "checker-s1",
        "specs--alpha",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Verification),
        None,
    );
    check_as(
        &mut engine,
        "checker-s2",
        "specs--alpha",
        Verdict::Failed,
        RecordKind::Engine(CheckKind::Verification),
        Some("found a gap"),
    );
    check_as(
        &mut engine,
        "grader-s3",
        "specs--alpha",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Conformance),
        None,
    );
    check_as(
        &mut engine,
        "attacker-m",
        "specs--alpha",
        Verdict::Ok,
        RecordKind::Foreign("x-audit".to_string()),
        None,
    );
    check_as(
        &mut engine,
        "checker-s1",
        "specs--beta",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Verification),
        None,
    );
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let mounted = Engine::from_archive_bytes(bytes).unwrap();

    let prov = mounted.entity_provenance("specs", "specs--alpha").unwrap();
    assert_eq!(prov.check_state, "check_failed");
    let last = prov
        .last_check
        .as_ref()
        .expect("the sealed record is served");
    assert_eq!(last.identity.as_deref(), Some("checker-s2"));
    assert_eq!(last.method.as_deref(), Some("found a gap"));
    assert_eq!(last.entity, "specs--alpha");
    assert_eq!(prov.conformance_state, "checked_ok");
    assert_eq!(
        prov.last_conformance_check
            .as_ref()
            .and_then(|r| r.identity.as_deref()),
        Some("grader-s3")
    );
    assert_eq!(prov.foreign_checks.len(), 1);
    assert_eq!(prov.foreign_checks[0].kind.as_deref(), Some("x-audit"));
    assert_eq!(
        prov.foreign_checks[0].identity.as_deref(),
        Some("attacker-m")
    );
    let sealed = prov.sealed.as_ref().unwrap();
    assert!(sealed.checks_carried);
    assert!(sealed.checks_reason.is_none());

    let prov = mounted.entity_provenance("specs", "specs--beta").unwrap();
    assert_eq!(prov.check_state, "checked_ok");
    assert_eq!(prov.conformance_state, "never_checked");
    assert!(prov.foreign_checks.is_empty());

    // The same derivation, in the JSON the MCP read serialises.
    let json = serde_json::to_value(&prov).unwrap();
    assert_eq!(json["check_state"], "checked_ok");
    assert_eq!(json["last_check"]["identity"], "checker-s1");
    assert_eq!(json["sealed"]["checks_carried"], true);

    // The health checks axis counts the archive's entities.
    let axis = crate::ops::health::health_checks_axis(&mounted, Some("specs"));
    let m = &axis["specs"];
    assert_eq!(m["checked_ok"], 1, "{axis}");
    assert_eq!(m["check_failed"], 1, "{axis}");
    assert_eq!(m["never_checked"], 0, "{axis}");
    assert_eq!(m["check_stale"], 0, "{axis}");
    assert_eq!(m["conformance"]["checked_ok"], 1, "{axis}");
    assert_eq!(m["conformance"]["never_checked"], 1, "{axis}");
    assert_eq!(m["foreign_kinds"]["x-audit"], 1, "{axis}");
    assert_eq!(m["sealed"]["carried"], true, "{axis}");
    assert_eq!(
        m["independence"]["unconfirmable"]["items"][0], "specs--beta",
        "written under no identity, the sealed reading is unconfirmable: {axis}"
    );

    // The read-only refusal stands: nothing on the mount records a check.
    let mut mounted = mounted;
    let (actor, client) = cli_actor();
    let err = mounted
        .record_check(
            "specs",
            "specs--alpha",
            Verdict::Ok,
            CheckKind::Verification,
            None,
            actor,
            Some(&client),
        )
        .unwrap_err();
    assert_eq!(err.code(), "READ_ONLY_MOUNT");
}

/// The independence reading travels with the sealed record. An archive
/// mount has no provenance to derive it from, so before this every
/// sealed ok read `unconfirmable` and no transition gate could hold on
/// the mount. The export now seals the reading the source engine
/// derives per record (`independence` on the member), and the mount
/// serves it through the same `independence_of` the health axis and the
/// gate provider read: an entity written under `author-a` and checked
/// under `checker-b` reads `confirmed_independent`, the author's own
/// check reads `self_checked`, and a member an older writer sealed
/// (no field) keeps reading `unconfirmable`. The self-contained re-pack
/// keeps the field.
#[test]
fn export_seals_the_independence_reading_the_mount_serves() {
    use crate::check::{CheckKind, RecordKind, Verdict};
    use crate::engine::independence::Independence;
    let tmp = TempDir::new().unwrap();
    let (mut engine, _dir) = folder_mem_with_entities(&tmp, &[]);
    engine.set_workspace_root(tmp.path().to_path_buf());
    engine.set_identity(Some("author-a".to_string()));
    let (actor, client) = cli_actor();
    for t in ["Alpha", "Beta", "Gamma"] {
        engine
            .create_entity(empty_create_args("specs", t), actor, Some(&client), None)
            .unwrap();
    }
    let verification = || RecordKind::Engine(CheckKind::Verification);
    check_as(
        &mut engine,
        "checker-b",
        "specs--alpha",
        Verdict::Ok,
        verification(),
        None,
    );
    check_as(
        &mut engine,
        "author-a",
        "specs--beta",
        Verdict::Ok,
        verification(),
        None,
    );
    check_as(
        &mut engine,
        "checker-b",
        "specs--gamma",
        Verdict::Ok,
        verification(),
        None,
    );

    let items = |axis: &serde_json::Value, bucket: &str| -> Vec<String> {
        let mut v: Vec<String> = axis["specs"]["independence"][bucket]["items"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        v.sort();
        v
    };
    // The control: the live engine's reading, derived from the folder ledger.
    let live = crate::ops::health::health_checks_axis(&engine, Some("specs"));
    assert_eq!(
        items(&live, "confirmed_independent"),
        ["specs--alpha", "specs--gamma"],
        "{live}"
    );
    assert_eq!(items(&live, "self_checked"), ["specs--beta"], "{live}");

    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let sealed = sealed_checks_of(&bytes).expect("the archive carries the checks member");
    let reading = |slug: &str| {
        sealed
            .latest(slug, "verification")
            .unwrap()
            .independence
            .clone()
    };
    assert_eq!(reading("alpha").as_deref(), Some("confirmed_independent"));
    assert_eq!(reading("beta").as_deref(), Some("self_checked"));
    assert_eq!(reading("gamma").as_deref(), Some("confirmed_independent"));
    assert_eq!(
        sealed
            .latest("alpha", "verification")
            .unwrap()
            .sealed_independence(),
        Some(Independence::ConfirmedIndependent)
    );

    let assert_served = |bytes: Vec<u8>, label: &str| {
        let mounted = Engine::from_archive_bytes(bytes).unwrap();
        let axis = crate::ops::health::health_checks_axis(&mounted, Some("specs"));
        assert_eq!(axis["specs"]["checked_ok"], 3, "{label}: {axis}");
        assert_eq!(
            items(&axis, "confirmed_independent"),
            ["specs--alpha", "specs--gamma"],
            "{label}: {axis}"
        );
        assert_eq!(
            items(&axis, "self_checked"),
            ["specs--beta"],
            "{label}: {axis}"
        );
        assert!(items(&axis, "unconfirmable").is_empty(), "{label}: {axis}");
        assert_eq!(
            axis["specs"]["independence"]["readings"]["specs--alpha"][0]["reading"],
            "confirmed_independent",
            "{label}: {axis}"
        );
        // The gate provider reads the same sealed reading.
        let provider = mounted.check_standing_provider();
        let alpha = mounted
            .get_entity(&crate::EntityId::new("specs", "alpha"))
            .unwrap();
        let standing = provider(alpha, "verification");
        assert!(standing.confirms(), "{label}: {standing:?}");
        let beta = mounted
            .get_entity(&crate::EntityId::new("specs", "beta"))
            .unwrap();
        let standing = provider(beta, "verification");
        assert!(!standing.confirms(), "{label}: {standing:?}");
        assert_eq!(standing.label(), "self_checked", "{label}");
        assert_eq!(
            mounted.sealed_independence_of(
                "specs",
                &mounted
                    .latest_check_record("specs", "specs--beta", "verification")
                    .unwrap()
            ),
            Some(Independence::SelfChecked),
            "{label}"
        );
    };
    assert_served(bytes.clone(), "export");
    let self_contained = crate::validator::make_archive_self_contained(&bytes).unwrap();
    assert_served(self_contained.bytes, "self-contained");

    // A member an older writer sealed: the same records, no reading.
    let entries = extract_entries(&bytes, &ValidatorLimits::DEFAULT).unwrap();
    let mut older: serde_json::Value =
        serde_json::from_slice(entries.checks_bytes.as_deref().unwrap()).unwrap();
    for kinds in older["entities"].as_object_mut().unwrap().values_mut() {
        for rec in kinds.as_object_mut().unwrap().values_mut() {
            rec.as_object_mut().unwrap().remove("independence");
        }
    }
    let older_bytes = serde_json::to_vec_pretty(&older).unwrap();
    assert!(!String::from_utf8_lossy(&older_bytes).contains("independence"));
    let rewritten = rewrite_zip_member(&bytes, memstead_schema::ARCHIVE_CHECKS_PATH, &older_bytes);
    let mounted = Engine::from_archive_bytes(rewritten).unwrap();
    let axis = crate::ops::health::health_checks_axis(&mounted, Some("specs"));
    assert_eq!(axis["specs"]["checked_ok"], 3, "{axis}");
    assert!(items(&axis, "confirmed_independent").is_empty(), "{axis}");
    assert!(items(&axis, "self_checked").is_empty(), "{axis}");
    assert_eq!(
        items(&axis, "unconfirmable"),
        ["specs--alpha", "specs--beta", "specs--gamma"],
        "{axis}"
    );
    let alpha = mounted
        .get_entity(&crate::EntityId::new("specs", "alpha"))
        .unwrap();
    let standing = (mounted.check_standing_provider())(alpha, "verification");
    assert!(!standing.confirms(), "{standing:?}");
    assert_eq!(standing.label(), "unconfirmable");
}

/// AC3 complement: a sealed record whose hash differs from the sealed
/// entity (the entity was edited after the check) reads stale, never
/// fresh, on the mount.
#[test]
fn sealed_record_over_a_later_edit_reads_stale_never_fresh() {
    use crate::check::{CheckKind, RecordKind, Verdict};
    let tmp = TempDir::new().unwrap();
    let mut engine = ledgered_folder_mem(&tmp, &["Alpha"]);
    check_as(
        &mut engine,
        "checker-s1",
        "specs--alpha",
        Verdict::Ok,
        RecordKind::Engine(CheckKind::Verification),
        None,
    );
    let (actor, client) = cli_actor();
    engine
        .update_entity(
            crate::UpdateEntityArgs {
                id: crate::EntityId::new("specs", "alpha"),
                expected_hash: None,
                sections: [("identity".to_string(), "edited after the check".to_string())]
                    .into_iter()
                    .collect(),
                append_sections: Default::default(),
                patch_sections: Default::default(),
                sections_unset: Vec::new(),
                metadata: Default::default(),
                metadata_unset: Vec::new(),
                dry_run: false,
                declare_relations: Vec::new(),
                anchors: Vec::new(),
                anchors_unset: Vec::new(),
                relations_unset: Vec::new(),
            },
            actor,
            Some(&client),
            None,
        )
        .unwrap();
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let mounted = Engine::from_archive_bytes(bytes).unwrap();
    let prov = mounted.entity_provenance("specs", "specs--alpha").unwrap();
    assert_eq!(prov.check_state, "check_stale");
    assert!(
        prov.last_check.is_some(),
        "the stale record is still served"
    );
    let axis = crate::ops::health::health_checks_axis(&mounted, Some("specs"));
    assert_eq!(axis["specs"]["check_stale"], 1, "{axis}");
    assert_eq!(axis["specs"]["checked_ok"], 0, "{axis}");
}

/// AC3 complement: an archive sealed without the member reads
/// `never_checked` with the reason stated, and a workspace ledger beside
/// the mount (holding a fresh ok record for the very id) is never
/// consulted for it.
#[test]
fn archive_mount_never_reads_the_workspace_ledger() {
    let tmp = TempDir::new().unwrap();
    let (engine, _dir) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let bytes = engine.export_mem_to_bytes("specs").unwrap();
    let mut mounted = Engine::from_archive_bytes(bytes).unwrap();
    let hash = mounted
        .get_entity(&crate::EntityId::new("specs", "alpha"))
        .unwrap()
        .content_hash
        .clone();
    let ws = TempDir::new().unwrap();
    crate::check::CheckLedger::for_workspace(ws.path())
        .record(&crate::check::CheckRecord {
            ts: 1,
            entity: "specs--alpha".to_string(),
            verdict: "ok".to_string(),
            method: None,
            entity_hash: hash,
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
            identity: Some("checker-beside".to_string()),
            kind: None,
            schema_ref: None,
            finding: None,
            renamed_from: None,
            carried_from: None,
        })
        .unwrap();
    mounted.set_workspace_root(ws.path().to_path_buf());

    let prov = mounted.entity_provenance("specs", "specs--alpha").unwrap();
    assert_eq!(prov.check_state, "never_checked");
    assert!(prov.last_check.is_none());
    let sealed = prov.sealed.as_ref().unwrap();
    assert!(!sealed.checks_carried);
    assert_eq!(
        sealed.checks_reason.as_deref(),
        Some("the archive carries no check records; every state reads never_checked")
    );
    let axis = crate::ops::health::health_checks_axis(&mounted, Some("specs"));
    assert_eq!(axis["specs"]["never_checked"], 1, "{axis}");
    assert_eq!(axis["specs"]["sealed"]["carried"], false, "{axis}");
    assert!(
        axis["specs"]["sealed"]["reason"]
            .as_str()
            .unwrap()
            .contains("no check records"),
        "{axis}"
    );
}

/// AC3 complement: a writable mem never reads a sealed member. A
/// `.memstead/checks.json` file dropped into a folder mem's directory is
/// not a source of check state; only the workspace ledger is.
#[test]
fn writable_mem_never_reads_a_sealed_member() {
    let tmp = TempDir::new().unwrap();
    let (engine, dir) = folder_mem_with_entities(&tmp, &["Alpha"]);
    let hash = engine
        .get_entity(&crate::EntityId::new("specs", "alpha"))
        .unwrap()
        .content_hash
        .clone();
    let member = serde_json::json!({
        "version": 1,
        "entities": {"alpha": {"verification": {
            "ts": 1, "verdict": "ok", "entity_hash": hash, "actor": "cli",
            "role": "checker", "identity": "checker-planted"
        }}}
    });
    std::fs::write(
        dir.join(".memstead").join("checks.json"),
        member.to_string(),
    )
    .unwrap();
    let engine = Engine::from_mounts(vec![(
        folder_mount("specs", dir.clone()),
        Box::new(FilesystemBackend::new(dir)) as Box<dyn MemBackend>,
    )])
    .unwrap();
    assert!(engine.archive_checks_for("specs").is_none());
    let (state, rec) = engine.entity_check_state("specs", "specs--alpha").unwrap();
    assert_eq!(state, crate::check::CheckState::NeverChecked);
    assert!(rec.is_none());
}
