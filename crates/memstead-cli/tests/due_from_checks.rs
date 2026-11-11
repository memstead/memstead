//! A due axis dated by a check kind: the due date is the UTC date of the
//! newest confirming check's act plus `offset_days`, a later check moves
//! it, a transcription of an older check does not, and an entity whose
//! newest check of the kind does not confirm is due now with its standing
//! and never with an older check's date. One clock pin: `--today` and
//! `MEMSTEAD_TODAY` give the same brief, and `health --include due`
//! agrees under the pin.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

const MANIFEST: &str = "name: assets\nversion: 0.1.0\ndescription: check-dated due fixture\nwhen_to_use: tests\ntypes:\n  - asset\nrelationships:\n  mode: strict\n  definitions:\n    - name: _default\n      description: fallback\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n";

const ASSET: &str = "name: asset\ndescription: reviewed asset\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    default_value: active\n    enum_values: [active, retired]\ndue:\n  check_kind: x-review\n  offset_days: 730\n  status_field: status\n  open_values: [active]\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: _default\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\n  - status\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n";

fn ok(ws: &Path, args: &[&str]) -> std::process::Output {
    let out = memstead()
        .current_dir(ws)
        .env_remove("MEMSTEAD_TODAY")
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn workspace() -> TempDir {
    let ws = TempDir::new().unwrap();
    ok(
        ws.path(),
        &["init", "--name", "assets", "--schema", "default@1.3.0"],
    );
    let dir = ws
        .path()
        .join(".memstead")
        .join("schemas")
        .join("assets@0.1.0");
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(dir.join("schema.yaml"), MANIFEST).unwrap();
    fs::write(dir.join("types").join("asset.yaml"), ASSET).unwrap();
    ok(ws.path(), &["mem", "set-schema", "assets", "assets@0.1.0"]);
    ws
}

fn asset(ws: &Path, title: &str) {
    ok(
        ws,
        &[
            "create",
            "--type",
            "asset",
            "--title",
            title,
            "--section",
            "body=the pump",
            "--metadata",
            "status=active",
            "--identity",
            "author-a",
        ],
    );
}

fn review(ws: &Path, id: &str, verdict: &str, who: &str, at: &str) {
    ok(
        ws,
        &[
            "check",
            id,
            "--kind",
            "x-review",
            "--verdict",
            verdict,
            "--method",
            "inspected",
            "--identity",
            who,
            "--role",
            "checker",
            "--observed-at",
            at,
        ],
    );
}

/// The due brief as data, pinned by `--today` or by `MEMSTEAD_TODAY`.
fn brief(ws: &Path, today: &str, by_env: bool) -> serde_json::Value {
    let mut cmd = memstead();
    cmd.current_dir(ws).env_remove("MEMSTEAD_TODAY");
    if by_env {
        cmd.env("MEMSTEAD_TODAY", today)
            .args(["due", "--within", "3y", "--json", "--quiet"]);
    } else {
        cmd.args([
            "due", "--within", "3y", "--today", today, "--json", "--quiet",
        ]);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn dated(v: &serde_json::Value, id: &str) -> Option<String> {
    ["overdue", "due_soon"]
        .iter()
        .flat_map(|k| v[*k].as_array().cloned().unwrap_or_default())
        .find(|r| r["id"] == id)
        .map(|r| r["date"].as_str().unwrap().to_string())
}

fn due_now(v: &serde_json::Value, id: &str) -> Option<String> {
    v["due_now"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|r| r["id"] == id)
        .map(|r| r["standing"].as_str().unwrap().to_string())
}

#[test]
fn a_recorded_check_moves_the_due_date() {
    let ws = workspace();
    let root = ws.path();
    asset(root, "Pump");
    let id = "assets--pump";
    let today = "2027-12-01";

    // Never checked under the kind: due now, no date anywhere.
    let v = brief(root, today, false);
    assert_eq!(due_now(&v, id).as_deref(), Some("never_checked"), "{v}");
    assert_eq!(dated(&v, id), None, "{v}");

    // An independent ok check: the act's UTC date plus 730 days.
    review(root, id, "ok", "checker-c", "2026-01-10T12:00:00Z");
    for by_env in [false, true] {
        let v = brief(root, today, by_env);
        assert_eq!(
            dated(&v, id).as_deref(),
            Some("2028-01-10"),
            "env {by_env}: {v}"
        );
        assert_eq!(due_now(&v, id), None, "{v}");
    }
    let health = memstead()
        .current_dir(root)
        .env("MEMSTEAD_TODAY", today)
        .args(["health", "--include", "due", "--json", "--quiet"])
        .output()
        .unwrap();
    let h: serde_json::Value = serde_json::from_slice(&health.stdout).unwrap();
    assert_eq!(h["due"]["today"], today, "{h}");
    assert_eq!(h["due"]["due_soon"][0]["date"], "2028-01-10", "{h}");

    // A later independent ok check moves the date.
    review(root, id, "ok", "checker-d", "2026-06-01T09:00:00Z");
    assert_eq!(
        dated(&brief(root, today, false), id).as_deref(),
        Some("2028-05-31")
    );

    // A check transcribed afterwards with an earlier act leaves it.
    review(root, id, "ok", "checker-e", "2026-03-01T09:00:00Z");
    assert_eq!(
        dated(&brief(root, today, false), id).as_deref(),
        Some("2028-05-31")
    );
}

#[test]
fn a_newest_check_that_does_not_confirm_is_due_now_with_its_standing() {
    let ws = workspace();
    let root = ws.path();
    let today = "2027-12-01";
    for title in ["Failed", "Own", "Edited"] {
        asset(root, title);
        // Every entity once carried a confirming check: no case may fall
        // back to that older date.
        review(
            root,
            &format!("assets--{}", title.to_lowercase()),
            "ok",
            "checker-c",
            "2026-01-10T12:00:00Z",
        );
    }
    review(
        root,
        "assets--failed",
        "failed",
        "checker-d",
        "2026-02-01T09:00:00Z",
    );
    review(
        root,
        "assets--own",
        "ok",
        "author-a",
        "2026-02-01T09:00:00Z",
    );
    ok(
        root,
        &[
            "update",
            "assets--edited",
            "--section",
            "body=the pump, rebuilt",
            "--auto-hash",
            "--identity",
            "author-a",
        ],
    );
    let v = brief(root, today, false);
    for (id, standing) in [
        ("assets--failed", "check_failed"),
        ("assets--own", "self_checked"),
        ("assets--edited", "check_stale"),
    ] {
        assert_eq!(due_now(&v, id).as_deref(), Some(standing), "{id}: {v}");
        assert_eq!(dated(&v, id), None, "{id} keeps no older date: {v}");
    }
    let md =
        String::from_utf8_lossy(&ok(root, &["due", "--within", "3y", "--today", today]).stdout)
            .to_string();
    assert!(md.contains("3 entries (0 overdue, 3 due now)"), "{md}");
    assert!(
        md.contains("**due now**: no confirming `x-review` check (check_failed)"),
        "{md}"
    );
}
