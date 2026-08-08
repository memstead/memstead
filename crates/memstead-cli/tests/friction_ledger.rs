#![cfg(feature = "mem-repo")]
//! The friction ledger's CLI leg (agent-trust plan 08): a refused CLI
//! call appends one content-free entry, a successful one appends
//! nothing, `health --include friction` serves the summary and its
//! absence leaves health untouched, the distinctive refusal payload
//! never reaches the ledger, the ledger dir is self-ignoring, and an
//! unwritable ledger degrades without changing the refusal.
//!
//! The MCP leg — the same fixture driving both surfaces through the
//! real dispatch seam — lives in `memstead-mcp/tests/wire_shape.rs`
//! (`friction_ledger_records_both_surfaces_and_serves_the_axis`),
//! where the spawned server binary exercises `call_tool` end to end.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use memstead_base::friction::{FrictionLedger, friction_ledger_path};
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use serde_json::Value;
use tempfile::TempDir;

/// Same fixture shape as `write_commands.rs`: a `cli-write/` mem with
/// a real mem-repo.
fn make_workspace(tmp: &Path) -> PathBuf {
    let mem = tmp.join("cli-write");
    fs::create_dir_all(&mem).unwrap();
    let store = mem.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    init_real_mem_repo_from_disk(tmp, &[(&mem, "cli-write")]);
    tmp.to_path_buf()
}

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn parse_json(stdout_bytes: &[u8]) -> Value {
    let body = std::str::from_utf8(stdout_bytes).expect("stdout must be UTF-8");
    serde_json::from_str(body.trim()).unwrap_or_else(|e| {
        panic!("--json output must parse as JSON: {e}\n--- stdout ---\n{body}\n--- end ---")
    })
}

#[test]
fn cli_refusals_append_successes_do_not_and_health_serves_the_axis() {
    let tmp = TempDir::new().unwrap();
    let ws = make_workspace(tmp.path());
    let ledger = FrictionLedger::for_workspace(&ws);
    let distinctive = "XyZzY-distinctive-payload-9a7f";

    // Refused CLI call: unknown section, payload carries the
    // distinctive string that must never reach the ledger.
    memstead()
        .current_dir(&ws)
        .args([
            "--json",
            "create",
            "--title",
            "Alpha",
            "--type",
            "spec",
            "--section",
            &format!("bogus-section={distinctive}"),
        ])
        .assert()
        .failure();
    let entries = ledger.entries();
    assert_eq!(entries.len(), 1, "one entry per refused CLI call");
    assert_eq!(entries[0].surface, "cli");
    assert_eq!(entries[0].verb, "create");
    assert_eq!(entries[0].code, "UNKNOWN_SECTION");
    assert!(entries[0].ts > 0, "entry carries a timestamp");

    // Privacy (criterion 4): no ledger byte contains the payload.
    let raw = fs::read_to_string(friction_ledger_path(&ws)).unwrap_or_default();
    assert!(
        !raw.contains(distinctive),
        "ledger must never contain parameter content"
    );

    // A second refusal on a different verb for the per-verb axis.
    memstead()
        .current_dir(&ws)
        .args(["--json", "entity", "cli-write--does-not-exist"])
        .assert()
        .failure();
    assert_eq!(ledger.entries().len(), 2);

    // Successful CLI call appends nothing.
    memstead()
        .current_dir(&ws)
        .args([
            "create",
            "--title",
            "Gamma",
            "--type",
            "spec",
            "--section",
            "identity=I.",
            "--section",
            "purpose=P.",
        ])
        .assert()
        .success();
    assert_eq!(
        ledger.entries().len(),
        2,
        "a successful call appends nothing"
    );

    // `health --include friction` reports counts per code and per verb.
    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "health", "--include", "friction"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let health = parse_json(&out);
    assert_eq!(health["friction"]["total"], 2, "{health}");
    assert_eq!(health["friction"]["by_code"]["UNKNOWN_SECTION"], 1);
    assert_eq!(health["friction"]["by_code"]["ENTITY_NOT_FOUND"], 1);
    assert_eq!(health["friction"]["by_verb"]["cli:create"], 1);
    assert_eq!(health["friction"]["by_verb"]["cli:entity"], 1);
    assert_eq!(health["friction"]["recent_24h"]["total"], 2);

    // Without the include: no friction key — the axis is include-gated
    // and default health output carries nothing new.
    let out = memstead()
        .current_dir(&ws)
        .args(["--json", "health"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let plain = parse_json(&out);
    assert!(
        plain.get("friction").is_none(),
        "health without the include must not carry the axis"
    );

    // Complement: the ledger dir is self-ignoring, so no git checkout
    // ever sees ledger residue.
    let gitignore = friction_ledger_path(&ws)
        .parent()
        .unwrap()
        .join(".gitignore");
    assert_eq!(fs::read_to_string(gitignore).unwrap(), "*\n");
}

/// Criterion 3: an unwritable ledger location degrades to
/// not-recording WITHOUT changing the refusal returned — the same
/// typed envelope with and without a writable ledger.
#[test]
#[cfg(unix)]
fn unwritable_ledger_never_changes_the_refusal() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let ws = make_workspace(tmp.path());

    let refuse = || {
        let out = memstead()
            .current_dir(&ws)
            .args([
                "--json", "create", "--title", "Alpha", "--type", "spec", "--section",
                "bogus-section=x",
            ])
            .assert()
            .failure()
            .get_output()
            .stdout
            .clone();
        let v = parse_json(&out);
        serde_json::json!({ "code": v["code"], "message": v["message"], "details": v["details"] })
    };

    // Writable: refusal recorded.
    let with_ledger = refuse();
    let ledger = FrictionLedger::for_workspace(&ws);
    assert_eq!(ledger.entries().len(), 1);

    // Seal the ledger dir (0555): appends must silently degrade.
    let dir = friction_ledger_path(&ws).parent().unwrap().to_path_buf();
    let file = friction_ledger_path(&ws);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let without_ledger = refuse();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(
        with_ledger, without_ledger,
        "the refusal envelope must be identical with and without a writable ledger"
    );
    assert_eq!(
        ledger.entries().len(),
        1,
        "the sealed window recorded nothing"
    );
}
