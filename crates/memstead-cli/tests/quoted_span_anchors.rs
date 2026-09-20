//! Quoted-span anchors through the CLI element (`--anchor <JSON>` with
//! `span`), the observation run (`verify-anchors --observations`), the
//! roster, the unset selector and the health anchors axis.
//!
//! A writer hands the engine the exact words an entity quotes beside the
//! text it read; the write refuses when the words are not in that text and
//! otherwise records the span with its hashes. From then on an observation
//! with `content` resolves the row while the words are present, whatever
//! else on the page changed, and reads `span_absent` once they are gone:
//! two agents' different extractions of one document no longer make each
//! other drift.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

const PAGE: &str = "https://example.test/report.pdf";
const TARIFF: &str = "notes--the-tariff-rose";
const BOARD: &str = "notes--the-board-refused";

fn create(ws: &Path, title: &str, claim: &str, anchor: &str) {
    memstead()
        .current_dir(ws)
        .args([
            "create",
            "--type",
            "assertion",
            "--title",
            title,
            "--section",
            &format!("claim={claim}"),
            "--section",
            "evidence=The report says so.",
            "--identity",
            "author-one",
            "--role",
            "author",
            "--quiet",
            "--anchor",
            anchor,
        ])
        .assert()
        .success();
}

/// A workspace whose `notes` mem carries two claims on one document, each
/// written from a different extraction of it.
fn workspace() -> TempDir {
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
    create(
        ws.path(),
        "The tariff rose",
        "The tariff rose to 12,5 % in March 2026.",
        &format!(
            r#"{{"artifact":"{PAGE}","grain":"url","class":"anchored","span":"rose to 12,5 % in March 2026","content":"REPORT 2026\nThe tariff rose to 12,5 % in\nMarch 2026. Footer a."}}"#
        ),
    );
    create(
        ws.path(),
        "The board refused",
        "The board did not approve the tariff.",
        &format!(
            r#"{{"artifact":"{PAGE}","grain":"url","class":"anchored","span":"board did not approve it","content":"Report 2026 - The tariff rose to 12,5 % in March 2026.\nThe board did not approve it.\nFooter b."}}"#
        ),
    );
    ws
}

fn verify(ws: &Path, observations: Option<&Path>) -> serde_json::Value {
    let mut cmd = memstead();
    cmd.current_dir(ws)
        .args(["verify-anchors", "--mem", "notes", "--json", "--quiet"]);
    if let Some(obs) = observations {
        cmd.arg("--observations").arg(obs);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn row<'a>(v: &'a serde_json::Value, entity: &str) -> &'a serde_json::Value {
    v["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["entity_id"] == entity)
        .unwrap_or_else(|| panic!("no row for {entity} in {v}"))
}

/// AC1 through the CLI element: the write refuses, typed and naming the
/// span, when the words are not in the supplied content; a span beside a
/// hash refuses; a span on the entity grain refuses.
#[test]
fn the_write_refuses_a_span_the_content_does_not_carry() {
    let ws = workspace();
    let refused = |anchor: &str, needle: &str| {
        let out = memstead()
            .current_dir(ws.path())
            .args(["update", TARIFF, "--quiet", "--anchor", anchor])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(5),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("INVALID_ANCHOR"), "{err}");
        assert!(err.contains(needle), "{err}");
    };
    refused(
        &format!(
            r#"{{"artifact":"{PAGE}","grain":"url","class":"anchored","span":"rose to 12,6 %","content":"The tariff rose to 12,5 %."}}"#
        ),
        "rose to 12,6 %",
    );
    refused(
        &format!(
            r#"{{"artifact":"{PAGE}","grain":"url","class":"anchored","span":"rose","hash":"abc"}}"#
        ),
        "both `span` and `hash`",
    );
    refused(
        r#"{"artifact":"notes--the-board-refused","grain":"entity","class":"anchored","span":"rose"}"#,
        "does not accept `span`",
    );
    // Nothing was written: the roster still holds the two rows.
    let out = memstead()
        .current_dir(ws.path())
        .args(["anchors", "--mem", "notes", "--json", "--quiet"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["count"], 2, "{v}");
}

/// AC2 and AC4 through the CLI: one observation of a third extraction
/// resolves both span rows (each entity pinned a different document hash),
/// the words of one claim leaving the page reads `span_absent` on that row
/// alone (never `drifted`), the roster and the health axis show it, and
/// `--anchor-unset` with a span removes one row and leaves the other.
#[test]
fn two_extractions_resolve_from_one_observation_and_a_lost_span_reads_span_absent() {
    let ws = workspace();
    // Both rows carry their span and a document hash; the hashes differ.
    let out = memstead()
        .current_dir(ws.path())
        .args(["anchors", "--mem", "notes", "--json", "--quiet"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(row(&v, TARIFF)["span"], "rose to 12,5 % in March 2026");
    assert_eq!(row(&v, BOARD)["span"], "board did not approve it");
    assert_ne!(row(&v, TARIFF)["hash"], row(&v, BOARD)["hash"]);
    assert!(row(&v, TARIFF)["span_hash"].is_string());
    assert_eq!(row(&v, TARIFF)["hash_source"], "author");
    let sidecar: serde_json::Value = serde_json::from_slice(
        &fs::read(ws.path().join(".memstead").join("anchors.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(sidecar["version"], 3);

    let obs = ws.path().join("obs.json");
    fs::write(
        &obs,
        format!(
            r#"[{{"artifact":"{PAGE}","content":"Report 2026\n\nThe tariff rose to\n12,5 % in March 2026. The board\ndid not approve it.\n\nFooter c."}}]"#
        ),
    )
    .unwrap();
    let v = verify(ws.path(), Some(&obs));
    assert_eq!(row(&v, TARIFF)["state"], "resolves", "{v}");
    assert_eq!(row(&v, BOARD)["state"], "resolves", "{v}");
    assert_eq!(v["drifted"], 0);
    assert_eq!(v["span_absent"], 0);
    assert_eq!(v["observations"]["recorded"], 2, "{v}");

    // The words of the first claim leave the page.
    fs::write(
        &obs,
        format!(
            r#"[{{"artifact":"{PAGE}","content":"Report 2026, corrected: the tariff rose to 12,6 % in March 2026. The board did not approve it."}}]"#
        ),
    )
    .unwrap();
    let v = verify(ws.path(), Some(&obs));
    assert_eq!(row(&v, TARIFF)["state"], "span_absent", "{v}");
    assert_eq!(row(&v, BOARD)["state"], "resolves", "{v}");
    assert_eq!(v["span_absent"], 1);
    assert_eq!(v["drifted"], 0);
    // The markdown rendering names the state and the span.
    let md = memstead()
        .current_dir(ws.path())
        .args(["verify-anchors", "--mem", "notes", "--quiet"])
        .output()
        .unwrap();
    let md = String::from_utf8_lossy(&md.stdout);
    assert!(md.contains("Span absent (quoted words gone): 1"), "{md}");
    assert!(
        md.contains("**span_absent**") && md.contains("rose to 12,5 % in March 2026"),
        "{md}"
    );
    // The recorded state shows on the roster and on the health axis.
    let out = memstead()
        .current_dir(ws.path())
        .args(["anchors", "--mem", "notes", "--json", "--quiet"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(row(&v, TARIFF)["state"], "span_absent", "{v}");
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "health",
            "--include",
            "anchors,open_questions",
            "--json",
            "--quiet",
        ])
        .output()
        .unwrap();
    let h: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(h["anchors"]["notes"]["span_absent"], 1, "{h}");
    assert_eq!(
        h["open_questions"]["notes"]["anchors_span_absent"]["count"], 1,
        "{h}"
    );

    // Unset by artifact and span: the other entity's row stays.
    memstead()
        .current_dir(ws.path())
        .args([
            "update",
            TARIFF,
            "--quiet",
            "--anchor-unset",
            &format!(r#"{{"artifact":"{PAGE}","span":"rose to  12,5 % in March 2026"}}"#),
        ])
        .assert()
        .success();
    let out = memstead()
        .current_dir(ws.path())
        .args(["anchors", "--mem", "notes", "--json", "--quiet"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["count"], 1, "{v}");
    assert_eq!(row(&v, BOARD)["span"], "board did not approve it");
}

/// AC2 refusal complement through the CLI: an `absent` observation reads
/// `recheck`, a `hash` observation cannot adjudicate a span row and leaves
/// it as it was (unobserved here, and reported as not applied).
#[test]
fn absent_reads_recheck_and_a_hash_observation_leaves_a_span_row_as_it_was() {
    let ws = workspace();
    let obs = ws.path().join("obs.json");
    fs::write(&obs, format!(r#"[{{"artifact":"{PAGE}","hash":"zz"}}]"#)).unwrap();
    let v = verify(ws.path(), Some(&obs));
    assert_eq!(row(&v, TARIFF)["state"], "unobserved", "{v}");
    assert_eq!(v["observations"]["recorded"], 0);
    assert_eq!(v["observations"]["unmatched"][0], PAGE);

    fs::write(&obs, format!(r#"[{{"artifact":"{PAGE}","absent":true}}]"#)).unwrap();
    let v = verify(ws.path(), Some(&obs));
    assert_eq!(row(&v, TARIFF)["state"], "recheck", "{v}");
    assert_eq!(row(&v, BOARD)["state"], "recheck", "{v}");
}

/// The record seam through the CLI: two span rows and a span-less row on
/// one url artifact, on one entity. After a mixed observation each row
/// carries its own recorded state on `anchors <id>`, and `health --include
/// anchors` counts one span_absent, as the verify did; a hash-only
/// observation afterwards leaves the span rows exactly as they were.
#[test]
fn a_recorded_observation_lands_on_the_one_row_it_adjudicated() {
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
    let page = "https://example.test/page";
    let text = "Alpha words here. Beta words there.";
    let mk = |span: Option<&str>| match span {
        Some(s) => format!(
            r#"{{"artifact":"{page}","grain":"url","class":"anchored","span":"{s}","content":"{text}"}}"#
        ),
        None => format!(
            r#"{{"artifact":"{page}","grain":"url","class":"anchored","content":"{text}"}}"#
        ),
    };
    memstead()
        .current_dir(ws.path())
        .args([
            "create",
            "--type",
            "assertion",
            "--title",
            "Three rows",
            "--section",
            "claim=Alpha and beta.",
            "--section",
            "evidence=The page.",
            "--identity",
            "author-one",
            "--role",
            "author",
            "--quiet",
            "--anchor",
            &mk(Some("Alpha words")),
            "--anchor",
            &mk(Some("Beta words")),
            "--anchor",
            &mk(None),
        ])
        .assert()
        .success();
    let entity = "notes--three-rows";
    let rows_of = |ws: &Path| -> serde_json::Value {
        let out = memstead()
            .current_dir(ws)
            .args(["anchors", entity, "--json", "--quiet"])
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let row = |v: &serde_json::Value, span: Option<&str>| -> serde_json::Value {
        v["anchors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["span"].as_str() == span)
            .cloned()
            .unwrap_or_else(|| panic!("no row for {span:?} in {v}"))
    };

    let obs = ws.path().join("obs.json");
    fs::write(
        &obs,
        format!(r#"[{{"artifact":"{page}","content":"Alpha words here. Gamma words there."}}]"#),
    )
    .unwrap();
    let v = verify(ws.path(), Some(&obs));
    // The span-less row reads by the whole-document rule: the url default
    // is `unstable`, so a changed page is recheck, not drifted.
    assert_eq!(
        (
            v["span_absent"].as_u64(),
            v["recheck"].as_u64(),
            v["drifted"].as_u64()
        ),
        (Some(1), Some(1), Some(0)),
        "{v}"
    );
    assert_eq!(v["observations"]["recorded"], 3, "{v}");
    let rows = rows_of(ws.path());
    assert_eq!(
        row(&rows, Some("Alpha words"))["last_observed"]["state"],
        "resolves"
    );
    assert_eq!(
        row(&rows, Some("Beta words"))["last_observed"]["state"],
        "span_absent"
    );
    assert_eq!(row(&rows, None)["last_observed"]["state"], "recheck");
    let health = |ws: &Path| -> serde_json::Value {
        let out = memstead()
            .current_dir(ws)
            .args([
                "health",
                "--include",
                "anchors,open_questions",
                "--json",
                "--quiet",
            ])
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let h = health(ws.path());
    assert_eq!(h["anchors"]["notes"]["span_absent"], 1, "{h}");
    assert_eq!(
        h["open_questions"]["notes"]["anchors_span_absent"]["count"], 1,
        "{h}"
    );

    // Hash-only afterwards: the span rows keep their records, the
    // span-less row takes the new one.
    fs::write(&obs, format!(r#"[{{"artifact":"{page}","hash":"zz"}}]"#)).unwrap();
    let v = verify(ws.path(), Some(&obs));
    assert_eq!(v["observations"]["recorded"], 1, "{v}");
    let later = rows_of(ws.path());
    for span in [Some("Alpha words"), Some("Beta words")] {
        assert_eq!(row(&later, span), row(&rows, span), "span row untouched");
    }
    assert_eq!(row(&later, None)["last_observed"]["hash"], "zz");
    let h = health(ws.path());
    assert_eq!(h["anchors"]["notes"]["span_absent"], 1, "{h}");
}
