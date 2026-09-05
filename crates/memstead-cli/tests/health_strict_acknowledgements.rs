//! `memstead health --strict` as the graph's referee: it evaluates the
//! strict set without an `--include` list, refuses on an unacknowledged
//! finding, honours an acknowledgement (a `failed` check record naming the
//! condition), and reports `STALE_ACKNOWLEDGEMENT` when the finding stops
//! occurring while the acknowledgement stands. The finding-code namespace
//! is enforced at `memstead check`.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// One entity file of the built-in `default@1.0.0` schema, with the given
/// identity body and an optional relationships block.
fn entity(title: &str, identity: &str, relationships: &str) -> String {
    format!(
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\n{identity}\n\n## Purpose\n\nA fixture.\n{relationships}"
    )
}

/// A folder-mount workspace with one mem `hold` carrying two defects: a
/// relationships row naming an entity of a mem that is not mounted
/// (`alpha`, `DANGLING_RELATION_TARGET_MISSING` — a same-mem row would
/// stub its target on load and read as `UNRESOLVED_STUB` instead) and a
/// body link into nothing (`beta`, `DANGLING_LINK_TARGET_MISSING`).
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let store = root.join(".memstead");
    fs::create_dir_all(store.join("state")).unwrap();
    fs::write(
        store.join("workspace.toml"),
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    )
    .unwrap();
    fs::write(
        store.join("state/mounts.json"),
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"hold","schema":"default@1.0.0","storage":{"type":"folder","path":"hold-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    )
    .unwrap();
    let mem = root.join("hold-mem");
    fs::create_dir_all(mem.join(".memstead")).unwrap();
    fs::write(
        mem.join(".memstead/config.json"),
        r#"{"format":1,"schema":"default@1.0.0"}"#,
    )
    .unwrap();
    fs::write(
        mem.join("alpha.md"),
        entity(
            "Alpha",
            "Depends on a row that names nothing.",
            "\n## Relationships\n\n- **DEPENDS_ON**: [[other--thing]]\n",
        ),
    )
    .unwrap();
    fs::write(
        mem.join("beta.md"),
        entity("Beta", "Points at [[gone]] on purpose.", ""),
    )
    .unwrap();
    tmp
}

/// `health --strict --json`: exit code, the report, the envelope line.
fn strict(root: &Path) -> (i32, Value, String) {
    let out = memstead()
        .current_dir(root)
        .args(["--json", "health", "--strict"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let split = stdout.find("\n{\"code\":\"HEALTH_STRICT_VIOLATIONS\"");
    let (report, envelope) = match split {
        Some(i) => (&stdout[..i], stdout[i..].trim().to_string()),
        None => (stdout.as_str(), String::new()),
    };
    let json: Value = serde_json::from_str(report)
        .unwrap_or_else(|e| panic!("health --json prints a report: {e}\n{stdout}"));
    (out.status.code().unwrap_or(-1), json, envelope)
}

fn check(root: &Path, args: &[&str]) -> (i32, Value) {
    let out = memstead()
        .current_dir(root)
        .args(["--json", "check"])
        .args(args)
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap_or(-1), json)
}

const ACK_METHOD: &str = "owner: the work-down lane; plan: backlog-repairs";

/// The whole cycle, in one workspace: refused unacknowledged, green
/// acknowledged, refused stale, green withdrawn.
#[test]
fn strict_honours_acknowledgements_and_flags_stale_ones() {
    let tmp = workspace();
    let root = tmp.path();

    // (1) Two unacknowledged findings, no --include given: refused, and
    //     the report names the evaluated set and both conditions.
    let (code, json, envelope) = strict(root);
    assert_eq!(code, 1, "{envelope}");
    let axis = &json["strict"];
    assert_eq!(
        axis["evaluated"],
        serde_json::json!([
            "integrity",
            "anchors",
            "stale",
            "missing_required_outgoing",
            "constraints",
            "signals"
        ])
    );
    let codes: Vec<&str> = axis["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(
        codes.contains(&"DANGLING_RELATION_TARGET_MISSING"),
        "{codes:?}"
    );
    assert!(codes.contains(&"DANGLING_LINK_TARGET_MISSING"), "{codes:?}");
    assert_eq!(axis["acknowledged"], 0);
    assert!(
        envelope.contains("DANGLING_RELATION_TARGET_MISSING: 1")
            && envelope.contains("DANGLING_LINK_TARGET_MISSING: 1"),
        "{envelope}"
    );

    // (2) Without --strict the same run exits 0 and the conditions ride
    //     the report as advisory, with no strict axis.
    let out = memstead()
        .current_dir(root)
        .args(["--json", "health", "--include", "integrity"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let advisory: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        advisory["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "DANGLING_RELATION_TARGET_MISSING"),
        "{advisory}"
    );
    assert!(advisory.get("strict").is_none());

    // (3) Acknowledge both: a failed check record naming the condition,
    //     with the owner and the closing plan in the method.
    for (id, cond) in [
        ("hold--alpha", "DANGLING_RELATION_TARGET_MISSING"),
        ("hold--beta", "DANGLING_LINK_TARGET_MISSING"),
    ] {
        let finding = format!(
            r#"{{"code":"{cond}","message":"known open, repaired by the plan named in method"}}"#
        );
        let (code, json) = check(
            root,
            &[
                id,
                "--verdict",
                "failed",
                "--finding",
                &finding,
                "--method",
                ACK_METHOD,
                "--identity",
                "checker-1",
                "--role",
                "checker",
            ],
        );
        assert_eq!(code, 0, "{json}");
    }
    let (code, json, envelope) = strict(root);
    assert_eq!(
        code, 0,
        "acknowledged findings do not fail the run: {envelope}"
    );
    let axis = &json["strict"];
    assert_eq!(axis["acknowledged"], 2, "{axis}");
    assert_eq!(axis["violations"], 0);
    for f in axis["findings"].as_array().unwrap() {
        assert_eq!(f["acknowledged"], true, "{f}");
        assert_eq!(f["acknowledgement"]["identity"], "checker-1");
        assert_eq!(f["acknowledgement"]["method"], ACK_METHOD);
    }
    // The markdown names the acknowledgement too.
    let md = memstead()
        .current_dir(root)
        .args(["health", "--strict"])
        .output()
        .unwrap();
    let md = String::from_utf8(md.stdout).unwrap();
    assert!(md.contains("## Strict"), "{md}");
    assert!(md.contains("acknowledged by checker-1"), "{md}");

    // (4) Repair beta while its acknowledgement stands: the finding is
    //     gone and the standing acknowledgement is the finding now,
    //     naming the record.
    fs::write(
        root.join("hold-mem/beta.md"),
        entity("Beta", "Points at nothing any more.", ""),
    )
    .unwrap();
    let (code, json, envelope) = strict(root);
    assert_eq!(code, 1, "{envelope}");
    let stale = json["strict"]["stale_acknowledgements"].as_array().unwrap();
    assert_eq!(stale.len(), 1, "{}", json["strict"]);
    assert_eq!(stale[0]["code"], "STALE_ACKNOWLEDGEMENT");
    assert_eq!(stale[0]["entity"], "hold--beta");
    assert_eq!(stale[0]["condition"], "DANGLING_LINK_TARGET_MISSING");
    assert_eq!(stale[0]["acknowledgement"]["identity"], "checker-1");
    assert_eq!(stale[0]["acknowledgement"]["method"], ACK_METHOD);
    assert!(envelope.contains("STALE_ACKNOWLEDGEMENT: 1"), "{envelope}");
    assert_eq!(json["strict"]["violations"], 1, "alpha stays acknowledged");

    // (5) Withdraw it with an ok check on the same condition: green again.
    let (code, json) = check(
        root,
        &[
            "hold--beta",
            "--verdict",
            "ok",
            "--finding",
            r#"{"code":"DANGLING_LINK_TARGET_MISSING","message":"repaired"}"#,
            "--identity",
            "checker-1",
            "--role",
            "checker",
        ],
    );
    assert_eq!(code, 0, "{json}");
    let (code, json, envelope) = strict(root);
    assert_eq!(code, 0, "{envelope}");
    assert!(
        json["strict"]["stale_acknowledgements"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// The finding-code namespace: `UPPER_SNAKE` must name a health
/// condition; a kebab-case code is the checker's own and records; an
/// acknowledgement without a method refuses.
#[test]
fn check_refuses_an_engine_shaped_code_that_names_no_condition() {
    let tmp = workspace();
    let root = tmp.path();

    let (code, json) = check(
        root,
        &[
            "hold--alpha",
            "--verdict",
            "failed",
            "--finding",
            r#"{"code":"NOT_A_CONDITION","message":"typo"}"#,
            "--method",
            ACK_METHOD,
        ],
    );
    assert_ne!(code, 0, "{json}");
    assert_eq!(json["code"], "INVALID_CHECK_FINDING", "{json}");
    assert!(
        json["message"]
            .as_str()
            .unwrap_or("")
            .contains("MISSING_REQUIRED_OUTGOING"),
        "the refusal names the vocabulary: {json}"
    );

    let (code, json) = check(
        root,
        &[
            "hold--alpha",
            "--verdict",
            "failed",
            "--finding",
            r#"{"code":"hidden-premise","message":"the checker's own code"}"#,
        ],
    );
    assert_eq!(code, 0, "own vocabulary records: {json}");

    let (code, json) = check(
        root,
        &[
            "hold--alpha",
            "--verdict",
            "failed",
            "--finding",
            r#"{"code":"DANGLING_RELATION_TARGET_MISSING","message":"no owner named"}"#,
        ],
    );
    assert_eq!(json["code"], "INVALID_CHECK_FINDING", "{json}");
    assert_ne!(code, 0);

    // Nothing above acknowledged anything: strict still refuses both.
    let (code, json, _) = strict(root);
    assert_eq!(code, 1);
    assert_eq!(json["strict"]["acknowledged"], 0);
}
