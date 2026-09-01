#![cfg(feature = "mem-repo")]
// `memstead install` is mem-repo-only; the lean build has no install to
// exercise, so the whole binary is skipped under
// `--no-default-features`.

//! A published mem installs on the strength of the schema it carries.
//!
//! Every archive embeds the schema it pins. These tests pin the
//! consequence: a mem published under a vocabulary the installing
//! workspace has never seen installs, mounts, and reads — offline, with
//! no prior `memstead schema install` — because the install stages the
//! archive's own schema package into the storage the pin resolver
//! reads.
//!
//! The two tiers must never collapse into one another, so both
//! polarities are asserted against the SAME package content: authoring
//! (`schema validate` / `schema install`) refuses a retired key loudly
//! so the author can act, while the same bytes sealed inside an archive
//! install and keep their written meaning — the installing user is not
//! the author and cannot fix a third party's sealed package.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use assert_cmd::Command;
use tempfile::TempDir;

/// Serializes the `MEMSTEAD_MEM_CACHE` env override across tests in
/// this binary — env mutation is process-global.
fn cache_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // A panicking test poisons the lock; the guarded state is the
    // process env, which the next test overwrites anyway — so recover
    // rather than cascade one real failure into five fake ones.
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The binary under test, with the operator role set: these fixtures
/// create mems, and mem creation is allowlist-gated in agent mode.
fn memstead() -> Command {
    let mut cmd = Command::cargo_bin("memstead").expect("memstead binary must be built by cargo");
    cmd.env("MEMSTEAD_OPERATOR_MODE", "1");
    cmd
}

fn run_ok(root: &Path, cache: &Path, args: &[&str]) -> Vec<u8> {
    memstead()
        .current_dir(root)
        .env("MEMSTEAD_MEM_CACHE", cache)
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone()
}

/// The schema's manifest. `fieldnotes` is deliberately not a built-in:
/// resolving it in the receiver workspace is only possible from what
/// the archive carries.
const MANIFEST: &str = r#"name: fieldnotes
version: 0.1.0
description: A third-party vocabulary the installing workspace has never seen.
when_to_use: In the embedded-schema install tests.
types:
  - note
relationships:
  mode: strict
  definitions:
    - name: FOLLOWS
      description: Sequential ordering between notes
      default_weight: 2.0
    - name: _default
      description: Fallback weight for unknown relationships
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;

/// The type file in the CURRENT schema language. `retired_key_variant`
/// below is the same content with `no_self_loop_relationships:`
/// spelled as the key it was renamed from, so both tiers can be
/// asserted against one package.
const NOTE_TYPE: &str = r#"name: note
description: One field note.
when_to_use: For anything observed in the field.
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules:
      - One paragraph of observation.
metadata_fields:
  - key: observer
    description: Who wrote the note down.
    field_type: string
    required: true
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: FOLLOWS
no_self_loop_relationships:
  - FOLLOWS
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
write_rules:
  - Keep it short.
"#;

/// The same type file spelled with the self-loop key retired on
/// 2026-08-08. An author writing this today is told to rename it; a
/// publisher who sealed it before the rename cannot be reached, so the
/// sealed copy keeps loading with its written meaning.
fn retired_selfloop_variant() -> String {
    NOTE_TYPE.replace("no_self_loop_relationships:", "propagating_relationships:")
}

/// The same type file spelled with the retired metadata-polarity key.
/// `optional: false` is the pre-flip way of writing `required: true`,
/// so a sealed copy must still resolve `observer` as required.
///
/// Pass `optional` to pick the polarity. BOTH are needed to prove the
/// key is read rather than dropped: an unmarked package resolves an
/// ABSENT key to required, so the `false` case alone passes whether
/// the key was honoured or silently discarded. Only the `true` case —
/// where honouring the key flips `observer` to optional — can tell the
/// two apart.
fn retired_optional_variant(optional: bool) -> String {
    let out = NOTE_TYPE.replace(
        "    field_type: string\n    required: true\n",
        &format!("    field_type: string\n    optional: {optional}\n"),
    );
    assert_ne!(out, NOTE_TYPE, "the polarity variant must actually differ");
    out
}

/// Write an authoring package directory carrying `note_type` as its
/// only type.
fn write_package(dir: &Path, note_type: &str) {
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(dir.join("schema.yaml"), MANIFEST).unwrap();
    fs::write(dir.join("types").join("note.yaml"), note_type).unwrap();
}

/// Build a publisher workspace holding one mem pinned to
/// `fieldnotes@0.1.0` with a single entity, and export it. Returns the
/// archive path. The schema is installed here and ONLY here — the
/// receiver never sees the authoring package.
fn publish_fieldnotes_archive(root: &Path, cache: &Path) -> PathBuf {
    run_ok(root, cache, &["mem-repo", "init", "."]);
    let pkg = root.join("fieldnotes-pkg");
    write_package(&pkg, NOTE_TYPE);
    run_ok(root, cache, &["schema", "install", pkg.to_str().unwrap()]);
    run_ok(
        root,
        cache,
        &[
            "mem",
            "init",
            "field-log",
            "--schema",
            "fieldnotes@0.1.0",
            "--no-gitignore",
        ],
    );
    run_ok(
        root,
        cache,
        &[
            "create",
            "--mem",
            "field-log",
            "--title",
            "Morning Count",
            "--type",
            "note",
            "--section",
            "body=Eleven herons on the east bank, just after first light.",
            "--metadata",
            "observer=A. Ranger",
        ],
    );

    let archive = root.join("field-log.mem");
    run_ok(
        root,
        cache,
        &[
            "export",
            "--format",
            "mem",
            "--mem",
            "field-log",
            "-o",
            archive.to_str().unwrap(),
        ],
    );
    assert!(archive.is_file(), "export must produce the archive");
    archive
}

/// `memstead schema <pin>` renders a WORKSPACE-INSTALLED package's
/// sealed README, not only a built-in's. The README is a contract
/// carrier (the flagship catalogue rides there), so the render verb
/// must read the same sealed store the installer wrote — refusing
/// with "no built-in schema" left workspace packages' READMEs without
/// a sanctioned read surface.
#[test]
fn schema_render_reads_workspace_installed_readme() {
    let _guard = cache_guard();
    let ws = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = ws.path();
    run_ok(root, cache.path(), &["mem-repo", "init", "."]);
    let pkg = root.join("fieldnotes-pkg");
    write_package(&pkg, NOTE_TYPE);
    fs::write(
        pkg.join("README.md"),
        "# fieldnotes\n\n<!-- CONTRACT:BEGIN -->\none testable line\n<!-- CONTRACT:END -->\n",
    )
    .unwrap();
    run_ok(
        root,
        cache.path(),
        &["schema", "install", pkg.to_str().unwrap()],
    );

    let out = run_ok(
        root,
        cache.path(),
        &["--json", "schema", "fieldnotes@0.1.0"],
    );
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["origin"], "workspace");
    let readme = v["readme"].as_str().expect("readme rendered");
    assert!(
        readme.contains("<!-- CONTRACT:BEGIN -->") && readme.contains("one testable line"),
        "sealed README content must round-trip: {readme}"
    );

    // A pin that exists nowhere still refuses, naming both stores.
    memstead()
        .current_dir(root)
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .args(["schema", "fieldnotes@9.9.9"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("SCHEMA_NOT_FOUND"));
}

/// A FOLDER mem inside a mem-repo workspace pins a schema that
/// `memstead schema install` sealed on the `__MEMSTEAD:schemas/` ref.
/// The loader resolves the pin from the ref, so the mem mounts and
/// writes — and export must read the SAME store. The historical
/// filesystem-only collector refused with `schema not found`, leaving
/// a mem that loads but cannot seal (found live on the flagship mem,
/// 2026-09-01: folder mount, pin on the ref, no filesystem package).
#[test]
fn folder_mem_with_ref_installed_schema_exports() {
    let _guard = cache_guard();
    let ws = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let root = ws.path();
    run_ok(root, cache.path(), &["mem-repo", "init", "."]);
    let pkg = root.join("fieldnotes-pkg");
    write_package(&pkg, NOTE_TYPE);
    run_ok(
        root,
        cache.path(),
        &["schema", "install", pkg.to_str().unwrap()],
    );
    run_ok(
        root,
        cache.path(),
        &[
            "mem",
            "init",
            "field-folder",
            "--schema",
            "fieldnotes@0.1.0",
            "--storage",
            "folder",
            "--no-gitignore",
        ],
    );
    run_ok(
        root,
        cache.path(),
        &[
            "create",
            "--mem",
            "field-folder",
            "--title",
            "Evening Count",
            "--type",
            "note",
            "--section",
            "body=Three herons at dusk, west bank.",
            "--metadata",
            "observer=A. Ranger",
        ],
    );

    let archive = root.join("field-folder.mem");
    run_ok(
        root,
        cache.path(),
        &[
            "export",
            "--format",
            "mem",
            "--mem",
            "field-folder",
            "-o",
            archive.to_str().unwrap(),
        ],
    );
    // The archive embeds the ref-sealed package — same member set the
    // git-branch export path seals for branch mems.
    let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    assert!(
        zip.by_name(NOTE_MEMBER).is_ok(),
        "archive must embed the ref-installed schema's type file"
    );
}

/// A receiver workspace: mem-repo shaped, one default-schema mem, and
/// no knowledge whatsoever of `fieldnotes`.
fn fresh_receiver(root: &Path, cache: &Path) {
    run_ok(root, cache, &["mem-repo", "init", "."]);
    run_ok(root, cache, &["mem", "init", "notes", "--no-gitignore"]);
}

/// The archive member holding the embedded type file.
const NOTE_MEMBER: &str = ".memstead/schema/types/note.yaml";
/// The archive member whose presence declares the package's
/// metadata-polarity generation.
const MARKER_MEMBER: &str = ".memstead/schema/schema-format.json";

/// Rewrite an archive, replacing one member's bytes and optionally
/// dropping others; every remaining member is copied through
/// byte-identical. This is how a genuinely pre-rename published
/// archive is reproduced: the publisher is unreachable, so the bytes
/// are what they are.
fn repack(src: &Path, dest: &Path, member: &str, new_bytes: &[u8], drop: &[&str]) {
    use std::io::{Read as _, Write as _};
    let mut archive = zip::ZipArchive::new(fs::File::open(src).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(fs::File::create(dest).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    let mut replaced = false;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if drop.contains(&name.as_str()) {
            continue;
        }
        writer.start_file(&name, opts).unwrap();
        if name == member {
            writer.write_all(new_bytes).unwrap();
            replaced = true;
        } else {
            writer.write_all(&bytes).unwrap();
        }
    }
    writer.finish().unwrap();
    assert!(replaced, "archive must carry the member {member}");
}

/// Copy an archive through, appending one extra member — how a
/// smuggling archive is built for whitelist complements.
fn append_member(src: &Path, dest: &Path, member: &str, bytes: &[u8]) {
    use std::io::{Read as _, Write as _};
    let mut archive = zip::ZipArchive::new(fs::File::open(src).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(fs::File::create(dest).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut body = Vec::new();
        entry.read_to_end(&mut body).unwrap();
        writer.start_file(&name, opts).unwrap();
        writer.write_all(&body).unwrap();
    }
    writer.start_file(member, opts).unwrap();
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap();
}

/// Backlog-sweep plan 09a criterion 1: a git-branch mem pinned to a
/// builtin whose install staged `mem-template.json` exports as `.mem`
/// (one representative per template-shipping family), the archive
/// re-reads cleanly in a fresh receiver, and the scaffolding did not
/// travel — the sealed schema an archive carries is the language, not
/// the install-time package. Complement: smuggling the template back
/// into the archive refuses at install — the whitelist did not widen.
#[test]
fn builtin_installs_with_scaffolding_still_export_and_reread() {
    let _guard = cache_guard();
    for schema_ref in [
        "engineering@0.1.0",
        "planning@0.4.0",
        "project@0.4.0",
        "software@0.4.0",
    ] {
        let sender = TempDir::new().unwrap();
        let receiver = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let root = sender.path();
        let name = schema_ref.split('@').next().unwrap();

        run_ok(root, cache.path(), &["mem-repo", "init", "."]);
        // The install stages the FULL builtin package — mem-template.json
        // included — onto the __MEMSTEAD ref; the export collector must
        // keep that scaffolding out of the archive.
        run_ok(root, cache.path(), &["schema", "install", schema_ref]);
        let mem = format!("{name}-mem");
        run_ok(
            root,
            cache.path(),
            &[
                "mem",
                "init",
                &mem,
                "--schema",
                schema_ref,
                "--no-gitignore",
            ],
        );
        let archive = root.join("out.mem");
        run_ok(
            root,
            cache.path(),
            &[
                "export",
                "--format",
                "mem",
                "--mem",
                &mem,
                "-o",
                archive.to_str().unwrap(),
            ],
        );

        // The sealed language travelled; the scaffolding did not.
        let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == ".memstead/schema/schema.yaml"),
            "{schema_ref}: sealed manifest must travel: {names:?}"
        );
        assert!(
            names.iter().all(|n| !n.contains("mem-template.json")),
            "{schema_ref}: install scaffolding must not travel: {names:?}"
        );

        // Re-read: a fresh receiver installs the archive cleanly and the
        // workspace still loads healthy with the new mount. (Builtins
        // resolve from the embedded catalogue, so no staging assertion —
        // the clean install + healthy load IS the re-read.)
        fresh_receiver(receiver.path(), cache.path());
        run_ok(
            receiver.path(),
            cache.path(),
            &["install", archive.to_str().unwrap()],
        );
        run_ok(receiver.path(), cache.path(), &["--json", "health"]);

        // Complement: the whitelist did not widen — an archive with the
        // template smuggled back refuses in a fresh receiver.
        let smuggled = root.join("smuggled.mem");
        append_member(
            &archive,
            &smuggled,
            ".memstead/schema/mem-template.json",
            b"{}",
        );
        let strict = TempDir::new().unwrap();
        fresh_receiver(strict.path(), cache.path());
        let out = memstead()
            .current_dir(strict.path())
            .env("MEMSTEAD_MEM_CACHE", cache.path())
            .args(["install", smuggled.to_str().unwrap()])
            .assert()
            .failure();
        let text = String::from_utf8_lossy(&out.get_output().stderr).to_string()
            + &String::from_utf8_lossy(&out.get_output().stdout);
        assert!(
            text.contains("unknown file"),
            "{schema_ref}: smuggled scaffolding must refuse as unknown file, got: {text}"
        );
    }
}

/// Read the schemas the receiver workspace now resolves from its own
/// local storage (the mem-repo's `__MEMSTEAD:schemas/` ref) — the
/// source the pin resolver consults, so what is readable here is what
/// a mount can be registered against.
fn staged_schemas(root: &Path) -> Vec<std::sync::Arc<memstead_schema::Schema>> {
    match memstead_git_branch::mem_repo_schemas::load_schemas_from_ref(root).unwrap() {
        memstead_git_branch::mem_repo_schemas::LoadOutcome::Schemas(s) => s,
        _ => Vec::new(),
    }
}

/// AC1 — a mem published under a schema the installing workspace has
/// never seen installs, mounts, and its entities are readable. No
/// network, no prior `schema install` in the receiver.
#[test]
fn archive_under_an_unknown_schema_installs_mounts_and_reads() {
    let _guard = cache_guard();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let archive = publish_fieldnotes_archive(sender.path(), cache.path());
    fresh_receiver(receiver.path(), cache.path());

    // Nothing named `fieldnotes` exists in the receiver before install.
    assert!(
        !staged_schemas(receiver.path())
            .iter()
            .any(|s| s.manifest.name == "fieldnotes"),
        "receiver must start with no knowledge of the publisher's schema"
    );

    run_ok(
        receiver.path(),
        cache.path(),
        &["install", archive.to_str().unwrap()],
    );

    // The mount is registered AND the entity reads back through it.
    let out = run_ok(
        receiver.path(),
        cache.path(),
        &["--json", "entity", "field-log--morning-count"],
    );
    let entity: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(entity["entity_type"], "note", "got: {entity}");
    assert!(
        entity.to_string().contains("Eleven herons"),
        "the installed mem's content must be readable: {entity}"
    );
}

/// AC2 — an archive whose embedded schema uses the retired
/// `propagating_relationships` key installs, and the key's written
/// meaning survives into the loaded schema.
#[test]
fn archive_with_a_retired_selfloop_key_installs_and_keeps_its_meaning() {
    let _guard = cache_guard();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let archive = publish_fieldnotes_archive(sender.path(), cache.path());
    let retired = sender.path().join("field-log-retired.mem");
    repack(
        &archive,
        &retired,
        NOTE_MEMBER,
        retired_selfloop_variant().as_bytes(),
        &[],
    );

    fresh_receiver(receiver.path(), cache.path());
    run_ok(
        receiver.path(),
        cache.path(),
        &["install", retired.to_str().unwrap()],
    );

    let staged = staged_schemas(receiver.path());
    let fieldnotes = staged
        .iter()
        .find(|s| s.manifest.name == "fieldnotes")
        .unwrap_or_else(|| panic!("staged schemas: {}", staged.len()));
    let note = fieldnotes.types.get("note").expect("type `note` must load");
    assert_eq!(
        note.no_self_loop_relationships,
        vec!["FOLLOWS".to_string()],
        "the retired key's written meaning must survive the rename"
    );

    // And the mem itself reads.
    let out = run_ok(
        receiver.path(),
        cache.path(),
        &["--json", "entity", "field-log--morning-count"],
    );
    let entity: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(entity["entity_type"], "note", "got: {entity}");
}

/// AC2, other retired key — a pre-flip archive writes `optional:`
/// where the current language writes `required:`. Such a package
/// carries no format marker (the marker and the polarity flip landed
/// together), so it reads under the generation it was sealed in.
///
/// BOTH polarities run, and the `true` case is the one that carries
/// the proof: an unmarked package resolves an ABSENT required/optional
/// key to required, so `optional: false` → required would hold equally
/// if the key were silently dropped. `optional: true` → optional can
/// only happen if the key was actually read and inverted.
#[test]
fn archive_with_a_retired_polarity_key_installs_and_keeps_its_meaning() {
    let _guard = cache_guard();
    let cache = TempDir::new().unwrap();

    for (optional, expect_required) in [(false, true), (true, false)] {
        let sender = TempDir::new().unwrap();
        let receiver = TempDir::new().unwrap();

        let archive = publish_fieldnotes_archive(sender.path(), cache.path());
        let preflip = sender.path().join("field-log-preflip.mem");
        repack(
            &archive,
            &preflip,
            NOTE_MEMBER,
            retired_optional_variant(optional).as_bytes(),
            &[MARKER_MEMBER],
        );

        fresh_receiver(receiver.path(), cache.path());
        run_ok(
            receiver.path(),
            cache.path(),
            &["install", preflip.to_str().unwrap()],
        );

        let staged = staged_schemas(receiver.path());
        let fieldnotes = staged
            .iter()
            .find(|s| s.manifest.name == "fieldnotes")
            .unwrap_or_else(|| panic!("staged schemas: {}", staged.len()));
        let note = fieldnotes.types.get("note").expect("type `note` must load");
        let observer = note
            .metadata_fields
            .iter()
            .find(|f| f.key == "observer")
            .expect("metadata field `observer` must load");
        assert_eq!(
            observer.required_resolved, expect_required,
            "`optional: {optional}` must invert to required={expect_required} — \
             the written meaning must survive the retirement"
        );
    }
}

/// AC3 — both polarities, one package, both retired keys. Authoring
/// refuses each retired key by name so the author can fix it; the same
/// content sealed in an archive installs. If the two tiers were ever
/// collapsed, one half of this test would fail.
#[test]
fn authoring_refuses_what_a_sealed_archive_still_admits() {
    let _guard = cache_guard();
    let sender = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    // --- Authoring half: the package directory refuses, loudly, for
    // BOTH retired keys and BOTH authoring verbs. ---
    let ws = sender.path();
    run_ok(ws, cache.path(), &["mem-repo", "init", "."]);

    let cases: [(&str, String, &str, &str); 2] = [
        (
            "selfloop",
            retired_selfloop_variant(),
            "propagating_relationships",
            "no_self_loop_relationships",
        ),
        (
            "polarity",
            retired_optional_variant(false),
            "optional",
            "required: true",
        ),
    ];
    for (label, content, retired_key, current_key) in &cases {
        let pkg = ws.join(format!("retired-{label}-pkg"));
        write_package(&pkg, content);
        for verb in ["validate", "install"] {
            let out = memstead()
                .current_dir(ws)
                .env("MEMSTEAD_MEM_CACHE", cache.path())
                .args(["--json", "schema", verb, pkg.to_str().unwrap()])
                .assert()
                .failure()
                .get_output()
                .stdout
                .clone();
            let envelope: serde_json::Value = serde_json::from_slice(&out).unwrap();
            let rendered = envelope.to_string();
            assert!(
                rendered.contains(retired_key),
                "`schema {verb}` must name the offending retired key \
                 `{retired_key}` — got: {rendered}"
            );
            assert!(
                rendered.contains(current_key),
                "`schema {verb}` must name the current spelling \
                 `{current_key}` so the author can act — got: {rendered}"
            );
        }
    }

    // --- Sealed half: the same content inside an archive installs.
    // Each sealed case gets its own receiver so neither can borrow the
    // other's staged schema. ---
    let publisher = TempDir::new().unwrap();
    let archive = publish_fieldnotes_archive(publisher.path(), cache.path());
    for (label, content, _, _) in &cases {
        let sealed = publisher.path().join(format!("sealed-{label}.mem"));
        // The polarity case reproduces a genuinely pre-flip package:
        // no format marker, because the marker postdates the flip.
        let drop: &[&str] = if *label == "polarity" {
            &[MARKER_MEMBER]
        } else {
            &[]
        };
        repack(&archive, &sealed, NOTE_MEMBER, content.as_bytes(), drop);

        let receiver = TempDir::new().unwrap();
        fresh_receiver(receiver.path(), cache.path());
        run_ok(
            receiver.path(),
            cache.path(),
            &["install", sealed.to_str().unwrap()],
        );
    }
}

/// AC4 — an archive whose embedded schema genuinely cannot be loaded
/// refuses under its own code, quotes the loader, does not send the
/// user off to obtain a package the archive contains, and leaves
/// neither a mount nor a staged schema behind.
#[test]
fn unloadable_embedded_schema_refuses_and_leaves_nothing_behind() {
    let _guard = cache_guard();
    let sender = TempDir::new().unwrap();
    let receiver = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    let archive = publish_fieldnotes_archive(sender.path(), cache.path());
    let broken = sender.path().join("broken.mem");
    repack(
        &archive,
        &broken,
        NOTE_MEMBER,
        b"name: note\nsections: [ this is not: valid: yaml\n",
        &[],
    );

    fresh_receiver(receiver.path(), cache.path());
    let out = memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .args(["--json", "install", broken.to_str().unwrap()])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let envelope: serde_json::Value = serde_json::from_slice(&out).unwrap();

    let code = envelope["code"].as_str().unwrap_or_default();
    assert_ne!(code, "SCHEMA_NOT_FOUND", "got: {envelope}");
    assert_ne!(code, "INTERNAL", "got: {envelope}");
    assert_eq!(code, "EMBEDDED_SCHEMA_INVALID", "got: {envelope}");

    let message = envelope["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("note.yaml") || message.contains("parse"),
        "the refusal must quote the loader's own diagnosis: {message}"
    );
    assert!(
        !message.contains("memstead schema install"),
        "the package is inside the archive — never advise obtaining it: {message}"
    );

    // Nothing mounted, nothing staged.
    memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .args(["--json", "entity", "field-log--morning-count"])
        .assert()
        .failure();
    assert!(
        !staged_schemas(receiver.path())
            .iter()
            .any(|s| s.manifest.name == "fieldnotes"),
        "a refused install must leave no staged schema"
    );
}

/// AC5 — installing the same mem twice is a no-op on the second run,
/// and two mems pinning the same schema install in either order.
#[test]
fn reinstall_is_a_noop_and_a_shared_schema_installs_in_either_order() {
    let _guard = cache_guard();
    let sender = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    // One publisher workspace, two mems on the same schema.
    let ws = sender.path();
    run_ok(ws, cache.path(), &["mem-repo", "init", "."]);
    let pkg = ws.join("fieldnotes-pkg");
    write_package(&pkg, NOTE_TYPE);
    run_ok(
        ws,
        cache.path(),
        &["schema", "install", pkg.to_str().unwrap()],
    );

    let mut archives = Vec::new();
    for mem in ["field-log", "tide-log"] {
        run_ok(
            ws,
            cache.path(),
            &[
                "mem",
                "init",
                mem,
                "--schema",
                "fieldnotes@0.1.0",
                "--no-gitignore",
            ],
        );
        run_ok(
            ws,
            cache.path(),
            &[
                "create",
                "--mem",
                mem,
                "--title",
                "First Entry",
                "--type",
                "note",
                "--section",
                "body=Something worth writing down.",
                "--metadata",
                "observer=A. Ranger",
            ],
        );
        let archive = ws.join(format!("{mem}.mem"));
        run_ok(
            ws,
            cache.path(),
            &[
                "export",
                "--format",
                "mem",
                "--mem",
                mem,
                "-o",
                archive.to_str().unwrap(),
            ],
        );
        archives.push(archive);
    }

    // Both orders, each in its own receiver.
    for order in [[0usize, 1usize], [1, 0]] {
        let receiver = TempDir::new().unwrap();
        fresh_receiver(receiver.path(), cache.path());
        for i in order {
            run_ok(
                receiver.path(),
                cache.path(),
                &["install", archives[i].to_str().unwrap()],
            );
        }
        // Both read back, whichever went first.
        for mem in ["field-log", "tide-log"] {
            run_ok(
                receiver.path(),
                cache.path(),
                &["--json", "entity", &format!("{mem}--first-entry")],
            );
        }
        // Exactly one staged copy of the shared schema.
        let staged = staged_schemas(receiver.path());
        assert_eq!(
            staged
                .iter()
                .filter(|s| s.manifest.name == "fieldnotes")
                .count(),
            1,
            "two mems sharing a schema stage it once"
        );
    }

    // Re-installing the same archive is a no-op on the mount side.
    let receiver = TempDir::new().unwrap();
    fresh_receiver(receiver.path(), cache.path());
    run_ok(
        receiver.path(),
        cache.path(),
        &["install", archives[0].to_str().unwrap()],
    );
    let out = memstead()
        .current_dir(receiver.path())
        .env("MEMSTEAD_MEM_CACHE", cache.path())
        .args(["--json", "install", archives[0].to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let payload: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(payload["mount"], "already_registered", "got: {payload}");
    assert_eq!(payload["copied_to_cache"], false, "got: {payload}");
}

/// The seal carries the SOURCE package's generation and never invents
/// one (backlog-sweep plan 05, decision 1). `schema install` of a
/// legacy builtin used to stamp the sealed copy with the
/// current-language marker, silently flipping every bare field from
/// required to optional the moment the sealed copy was read. Now: a
/// legacy builtin seals UNMARKED (absence IS its legacy claim, meaning
/// conserved — verified through a booted engine reading the sealed
/// copy); a current-generation builtin seals with its marker as-found;
/// and an authored directory package still receives the marker (the
/// resolver just verified it under the current language).
#[test]
fn seal_carries_source_generation_never_invents_the_marker() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let cache = root.join("cache");
    run_ok(root, &cache, &["mem-repo", "init", ".", "--no-gitignore"]);

    let ref_has = |pkg: &str| -> bool {
        std::process::Command::new("git")
            .arg("--git-dir")
            .arg(root.join("mem-repo").join(".git"))
            .args([
                "cat-file",
                "-e",
                &format!("__MEMSTEAD:schemas/{pkg}/schema-format.json"),
            ])
            .status()
            .unwrap()
            .success()
    };

    // Legacy builtin: sealed UNMARKED. (`engineering@0.1.0` is the
    // bare-field legacy class — no retired `optional:` keys, so it
    // passes the install validation gate; its bare fields carry the
    // pre-flip absent-key-means-required meaning.)
    run_ok(root, &cache, &["schema", "install", "engineering@0.1.0"]);
    assert!(
        !ref_has("engineering@0.1.0"),
        "a legacy builtin must seal without the current-language marker"
    );

    // Current-generation builtin: marker travels as-found.
    run_ok(root, &cache, &["schema", "install", "default@1.3.0"]);
    assert!(
        ref_has("default@1.3.0"),
        "a current-generation builtin's marker travels with the seal"
    );

    // Authored directory package: the resolver mints the marker
    // (complement — the fix does not unstamp genuine current content).
    let pkg = root.join("fieldnotes-pkg");
    write_package(&pkg, NOTE_TYPE);
    run_ok(root, &cache, &["schema", "install", pkg.to_str().unwrap()]);
    assert!(
        ref_has("fieldnotes@0.1.0"),
        "an authored (current-language) package seals marked"
    );

    // Round-trip meaning: a mem pinned to the sealed legacy package
    // reads it under LEGACY semantics — `decision.decided_on` is a
    // bare field (no required key), which meant REQUIRED pre-flip and
    // must still read as required from the sealed copy. A mis-stamped
    // (current-marked) seal would flip it to optional.
    run_ok(
        root,
        &cache,
        &[
            "workspace",
            "allow-create",
            "hold",
            "--schema",
            "engineering@0.1.0",
        ],
    );
    run_ok(
        root,
        &cache,
        &[
            "mem",
            "init",
            "hold",
            "--schema",
            "engineering@0.1.0",
            "--no-gitignore",
        ],
    );
    let out = run_ok(
        root,
        &cache,
        &["--json", "type", "decision", "--mem", "hold"],
    );
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        v["schema"], "engineering@0.1.0",
        "the sealed pin resolved: {v}"
    );
    let md = v["markdown"].as_str().expect("type description markdown");
    assert!(
        md.contains("**decided_on**: Date (required)"),
        "bare legacy field keeps its pre-flip REQUIRED meaning through the seal \
         (a mis-stamped current-marked seal would read it optional): {md}"
    );
    assert!(
        md.contains("**deciders**: String (required"),
        "second bare field likewise: {md}"
    );
}
