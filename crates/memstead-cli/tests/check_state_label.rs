//! `entity --provenance` names which check kind its state line reports.
//! The derived state is the `verification` kind's; a caller-declared
//! `x-<name>` check never moves it and is listed on its own line, so an
//! unlabelled "check state: never_checked" beside a recorded `x-` check
//! read as if the entity had never been checked at all.

use assert_cmd::Command;
use tempfile::TempDir;

fn ok(ws: &std::path::Path, args: &[&str]) -> String {
    let out = Command::cargo_bin("memstead")
        .unwrap()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`memstead {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_provenance_state_line_names_the_verification_kind() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    ok(ws, &["mem-repo", "init", "."]);
    ok(ws, &["workspace", "allow-create", "--schema", "*", "n*"]);
    ok(ws, &["mem", "init", "notes", "--schema", "default@1.3.0"]);
    ok(
        ws,
        &[
            "create",
            "--mem",
            "notes",
            "--title",
            "Alpha",
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=x",
            "--section",
            "purpose=y",
        ],
    );
    ok(
        ws,
        &[
            "check",
            "notes--alpha",
            "--kind",
            "x-review",
            "--verdict",
            "ok",
            "--method",
            "read",
            "--identity",
            "reviewer",
        ],
    );
    let text = ok(ws, &["entity", "notes--alpha", "--provenance"]);
    assert!(
        text.contains("- check state (verification): never_checked"),
        "{text}"
    );
    assert!(text.contains("last x-review check: ok"), "{text}");
}
