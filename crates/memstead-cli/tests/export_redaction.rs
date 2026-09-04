// `memstead mem init` (git-branch mems) ships only in the full build.

//! `memstead export --format mem` redacts private-pattern spans in the
//! archive's authoring provenance to `[redacted:<class>]`, never strips
//! the record, counts redactions per class in the report, and leaves
//! entity bodies alone (backlog-decisions plan B1). Proven on a folder mem
//! and on a git-branch mem, and the unpacked archive passes
//! `scripts/leak-scan.sh` with no allowlist.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// The private spans, assembled at runtime so this file carries none of
/// the leak scan's patterns as a literal.
fn private_note() -> String {
    format!(
        "see {}/x.md, checked at {}/w/notes",
        ["dev", "plans"].join("/"),
        ["/Users", "bjornbosenberg"].join("/"),
    )
}

fn create(ws: &Path, mem: &str, title: &str, body: &str, note: &str) {
    memstead()
        .current_dir(ws)
        .args([
            "create",
            "--quiet",
            "--mem",
            mem,
            "--type",
            "concept",
            "--title",
            title,
            "--section",
            &format!("definition={body}"),
            "--section",
            "explanation=x",
            "--note",
            note,
        ])
        .assert()
        .success();
}

/// A folder workspace (`memstead init`) with one mem `notes`.
fn folder_workspace() -> TempDir {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "init",
            "--name",
            "notes",
            "--schema",
            "default@1.3.0",
            "--quiet",
        ])
        .assert()
        .success();
    ws
}

/// A mem-repo workspace with one git-branch mem `notes`.
fn branch_workspace() -> TempDir {
    let ws = TempDir::new().unwrap();
    memstead()
        .current_dir(ws.path())
        .args(["mem-repo", "init", "--quiet", "--no-gitignore"])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "workspace",
            "allow-create",
            "notes",
            "--schema",
            "default@1.3.0",
            "--quiet",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "mem",
            "init",
            "notes",
            "--schema",
            "default@1.3.0",
            "--quiet",
        ])
        .assert()
        .success();
    ws
}

fn export(ws: &Path) -> (serde_json::Value, PathBuf) {
    let out_path = ws.join("notes.mem");
    let out = memstead()
        .current_dir(ws)
        .args([
            "export",
            "--format",
            "mem",
            "--mem",
            "notes",
            "-o",
            out_path.to_str().unwrap(),
            "--json",
            "--quiet",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (serde_json::from_slice(&out.stdout).unwrap(), out_path)
}

fn unpack(archive: &Path) -> TempDir {
    let dir = TempDir::new().unwrap();
    let file = fs::File::open(archive).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    for i in 0..zip.len() {
        let mut member = zip.by_index(i).unwrap();
        let target = dir.path().join(member.name());
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        fs::write(target, bytes).unwrap();
    }
    dir
}

fn leak_scan(dir: &Path) -> (bool, String) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/leak-scan.sh");
    let out = StdCommand::new("bash")
        .arg(&script)
        .arg(dir)
        .env_remove("LEAK_SCAN_EXTRA_ALLOW_FILE")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

fn assert_redacted(ws: &Path) {
    let (report, archive) = export(ws);
    let redactions = report["redactions"]
        .as_array()
        .expect("report lists redactions");
    let listed: Vec<(String, u64)> = redactions
        .iter()
        .map(|r| {
            (
                r["class"].as_str().unwrap().to_string(),
                r["count"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            ("absolute-user-paths".to_string(), 1),
            ("internal-refs".to_string(), 1),
        ],
        "{report}"
    );

    let unpacked = unpack(&archive);
    let prov: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(unpacked.path().join(".memstead/provenance.json")).unwrap(),
    )
    .unwrap();
    let entry = &prov["entities"]["alpha"];
    assert_eq!(
        entry["rationale"],
        "see [redacted:internal-refs]/x.md, checked at [redacted:absolute-user-paths]/w/notes"
    );
    // Redact, never strip: the row keeps its shape and its other fields.
    assert_eq!(entry["kind"], "create", "{prov}");
    assert!(entry["timestamp"].is_string(), "{prov}");
    assert!(entry["actor"].is_string(), "{prov}");

    let (clean, text) = leak_scan(unpacked.path());
    assert!(
        clean,
        "leak scan must be clean without any allowlist:\n{text}"
    );
}

/// B1 AC1 on a folder mem.
#[test]
fn folder_export_redacts_reports_and_scans_clean() {
    let ws = folder_workspace();
    create(ws.path(), "notes", "Alpha", "a clean body", &private_note());
    assert_redacted(ws.path());
}

/// B1 AC1 on a git-branch mem: the same builder, the same bytes.
#[test]
fn branch_export_redacts_reports_and_scans_clean() {
    let ws = branch_workspace();
    create(ws.path(), "notes", "Alpha", "a clean body", &private_note());
    assert_redacted(ws.path());
    // The markdown report carries the same counts.
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "export",
            "--format",
            "mem",
            "--mem",
            "notes",
            "-o",
            "again.mem",
            "--quiet",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("- Redacted in provenance: absolute-user-paths 1, internal-refs 1"),
        "{text}"
    );
}

/// B1 AC1 refusal complement, second half: entity bodies are not
/// rewritten, so the leak scan still flags them on the unpacked archive.
#[test]
fn entity_bodies_are_not_rewritten_and_still_fail_the_leak_scan() {
    let ws = folder_workspace();
    create(
        ws.path(),
        "notes",
        "Alpha",
        &private_note(),
        "an ordinary note",
    );
    let (report, archive) = export(ws.path());
    assert!(
        report["redactions"].as_array().is_none_or(|a| a.is_empty()),
        "no rationale carried a private span: {report}"
    );
    let unpacked = unpack(&archive);
    let body = fs::read_to_string(unpacked.path().join("alpha.md")).unwrap();
    assert!(body.contains(&["dev", "plans", "x.md"].join("/")), "{body}");
    let (clean, text) = leak_scan(unpacked.path());
    assert!(!clean, "the body still trips the scan:\n{text}");
    assert!(text.contains("internal-refs"), "{text}");
}
