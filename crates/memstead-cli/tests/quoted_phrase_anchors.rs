//! The `quoted-phrase` preparation and the anchors roster.
//!
//! A claims register anchors the words a surface asserts: under a source
//! declaring `quoted-phrase`, the artifact `<file>#<phrase>` resolves while
//! the file still carries the phrase and reads `orphaned` once the words are
//! gone, whatever else changed around them; a `url#<phrase>` row is
//! adjudicated the same way on observer-supplied content (the engine never
//! fetches), and `memstead anchors --mem <name> --grain url` is the roster
//! that observer works from. The binding verify counts every located span
//! and every url row of the binding's sources in its population and fails
//! `--fail-on-findings` on the orphaned ones.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

const ENTITY: &str = "notes--writes-are-refused-with-a-hint";
const SPAN: &str = "docs.txt#refused with a typed hint";
const PAGE: &str = "https://example.test/page#refused with a typed hint";
const WHOLE_PAGE: &str = "https://example.test/other";

/// A folder workspace whose `notes` mem carries one claim with three
/// anchors under a `quoted-phrase` source: a phrase span into `docs.txt`,
/// a phrase url row, and a whole-page url row.
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
    fs::write(
        ws.path().join("docs.txt"),
        "# Docs\n\nA nonconforming write is refused with a typed hint.\n",
    )
    .unwrap();
    memstead()
        .current_dir(ws.path())
        .args([
            "projection",
            "init",
            "--mem",
            "notes",
            "--name",
            "claims",
            "--source",
            ".",
            "--medium-type",
            "codebase",
            "--intent",
            "Claims about docs.txt and the live page: each claim anchors the words that assert it.",
            "--quiet",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "projection", "edit", "notes/claims", "--patch",
            r#"{"sources":[{"name":"docs","type":"codebase","pointer":".","preparation":"quoted-phrase","scope":[{"path":"docs.txt","mode":"allow"}]}]}"#,
            "--quiet",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "create",
            "--type",
            "assertion",
            "--title",
            "Writes are refused with a hint",
            "--section",
            "claim=A nonconforming write is refused.",
            "--section",
            "evidence=docs.txt says so.",
            "--identity",
            "author-one",
            "--role",
            "author",
            "--quiet",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(ws.path())
        .args([
            "update", ENTITY, "--quiet",
            "--anchor",
            &format!(r#"{{"artifact":"{SPAN}","grain":"span","class":"anchored","source":"docs"}}"#),
            "--anchor",
            &format!(r#"{{"artifact":"{PAGE}","grain":"url","class":"anchored","source":"docs","content":"<p>A nonconforming write is refused with a typed hint.</p>"}}"#),
            "--anchor",
            &format!(r#"{{"artifact":"{WHOLE_PAGE}","grain":"url","class":"anchored","source":"docs","content":"whole page"}}"#),
        ])
        .assert()
        .success();
    ws
}

fn states(ws: &Path, observations: Option<&Path>) -> Vec<(String, String)> {
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
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["artifact"].as_str().unwrap().to_string(),
                a["state"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn state_of(rows: &[(String, String)], artifact: &str) -> String {
    rows.iter()
        .find(|(a, _)| a == artifact)
        .map(|(_, s)| s.clone())
        .unwrap_or_else(|| panic!("no row for {artifact} in {rows:?}"))
}

#[test]
fn a_phrase_anchor_resolves_while_the_words_stand_and_orphans_when_they_leave() {
    let ws = workspace();
    let obs = ws.path().join("obs.json");
    fs::write(
        &obs,
        format!(
            r#"[{{"artifact":"{PAGE}","content":"<html>Every nonconforming write is refused with a typed hint, we promise.</html>"}},
                {{"artifact":"{WHOLE_PAGE}","content":"whole page"}}]"#
        ),
    )
    .unwrap();

    // First pass: the hash-less span is backfilled from the phrase; the url
    // rows adjudicate from the observations, the phrase row on its phrase
    // even though the page around it is a different page than at write time.
    let first = states(ws.path(), Some(&obs));
    assert_eq!(state_of(&first, PAGE), "resolves", "{first:?}");
    assert_eq!(state_of(&first, WHOLE_PAGE), "resolves", "{first:?}");
    let second = states(ws.path(), Some(&obs));
    assert_eq!(state_of(&second, SPAN), "resolves", "{second:?}");

    // A rewrite that keeps the words changes nothing.
    fs::write(
        ws.path().join("docs.txt"),
        "Preface.\n\nEvery nonconforming write is refused with a typed hint, we promise.\n",
    )
    .unwrap();
    let kept = states(ws.path(), Some(&obs));
    assert_eq!(state_of(&kept, SPAN), "resolves", "{kept:?}");

    // The words gone from the file and from the page: orphaned, not a
    // differing hash. The whole-page row under the url default (unstable)
    // reads recheck on a changed page, as before.
    fs::write(
        ws.path().join("docs.txt"),
        "# Docs\n\nA nonconforming write is coerced silently.\n",
    )
    .unwrap();
    let obs2 = ws.path().join("obs2.json");
    fs::write(
        &obs2,
        format!(
            r#"[{{"artifact":"{PAGE}","content":"<html>Writes are coerced.</html>"}},
                {{"artifact":"{WHOLE_PAGE}","content":"whole page changed"}}]"#
        ),
    )
    .unwrap();
    let gone = states(ws.path(), Some(&obs2));
    assert_eq!(state_of(&gone, SPAN), "orphaned", "{gone:?}");
    assert_eq!(state_of(&gone, PAGE), "orphaned", "{gone:?}");
    assert_eq!(state_of(&gone, WHOLE_PAGE), "recheck", "{gone:?}");

    // The binding verify counts all three rows in its population (a located
    // span and a url row were once silently "out of scope") and fails on the
    // two orphaned ones.
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "projection",
            "verify",
            "notes/claims",
            "--full",
            "--fail-on-findings",
            "--json",
            "--quiet",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(6),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // The report is followed by the findings refusal envelope on stdout, so
    // read the text rather than one JSON value.
    let text = String::from_utf8_lossy(&out.stdout).replace([' ', '\n'], "");
    assert!(text.contains(r#""counted_rows":3"#), "{text}");
    assert!(text.contains(r#""excluded_out_of_scope":0"#), "{text}");
    assert!(text.contains("unresolvable-anchor"), "{text}");
}

#[test]
fn the_roster_lists_one_mems_anchors_and_narrows_by_grain() {
    let ws = workspace();
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "anchors", "--mem", "notes", "--grain", "url", "--json", "--quiet",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["count"], 2, "{v}");
    assert_eq!(v["grain"], "url");
    assert_eq!(v["mem"], "notes");
    let artifacts: Vec<&str> = v["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["artifact"].as_str().unwrap())
        .collect();
    assert_eq!(artifacts, vec![PAGE, WHOLE_PAGE]);
    // Every row of the mem without the grain filter.
    let all = memstead()
        .current_dir(ws.path())
        .args(["anchors", "--mem", "notes", "--json", "--quiet"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&all.stdout).unwrap();
    assert_eq!(v["count"], 3, "{v}");
    // The markdown form names the mem and the grain when nothing matches.
    memstead()
        .current_dir(ws.path())
        .args(["anchors", "--mem", "notes", "--grain", "tree", "--quiet"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "No anchors of grain `tree` for mem `notes`",
        ));
    // A mem the workspace does not mount is a refusal, never an empty roster.
    memstead()
        .current_dir(ws.path())
        .args(["anchors", "--mem", "nope", "--quiet"])
        .assert()
        .code(3)
        .stderr(predicates::str::contains("MEM_NOT_FOUND"));
    // An unknown grain refuses with the allowed set.
    memstead()
        .current_dir(ws.path())
        .args([
            "anchors",
            "--mem",
            "notes",
            "--grain",
            "paragraph",
            "--quiet",
        ])
        .assert()
        .code(5)
        .stderr(predicates::str::contains("span, file, tree, url, entity"));
}

/// The entity side of coverage: a destination entity no anchor stands behind
/// is named as such by the fidelity report, and one declared to carry no
/// anchor on purpose (`projection exclude --entity-exclusions`) is named as
/// excluded with its reason instead — a withdrawn claim reads as a decision,
/// never as a gap. An id that is not an entity of the destination mem
/// refuses the whole call.
#[test]
fn an_entity_without_anchors_is_named_and_a_declared_one_is_excluded_with_its_reason() {
    let ws = workspace();
    memstead()
        .current_dir(ws.path())
        .args([
            "create",
            "--type",
            "assertion",
            "--title",
            "The retired app showed diffs",
            "--section",
            "claim=The retired app showed diffs you could undo.",
            "--section",
            "evidence=Withdrawn 2026-08-18 with its subject.",
            "--identity",
            "author-one",
            "--role",
            "author",
            "--quiet",
        ])
        .assert()
        .success();
    let verify = |ws: &Path| -> (String, serde_json::Value) {
        let out = memstead()
            .current_dir(ws)
            .args([
                "projection",
                "verify",
                "notes/claims",
                "--full",
                "--json",
                "--quiet",
            ])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let first: serde_json::Value = serde_json::Deserializer::from_str(&text)
            .into_iter::<serde_json::Value>()
            .next()
            .unwrap()
            .unwrap();
        (text, first)
    };
    let (_, report) = verify(ws.path());
    assert_eq!(
        report["report"]["coverage"]["unanchored_entities"],
        serde_json::json!(["notes--the-retired-app-showed-diffs"]),
        "{report}"
    );
    assert_eq!(report["report"]["coverage"]["excluded_entities"], 0);

    // A foreign or absent id refuses whole; nothing is written.
    memstead()
        .current_dir(ws.path())
        .args([
            "projection",
            "exclude",
            "notes/claims",
            "--entity-exclusions",
            r#"{"notes--the-retired-app-showed-diffs": "withdrawn", "other--nope": "x"}"#,
            "--quiet",
        ])
        .assert()
        .code(5)
        .stderr(predicates::str::contains(
            "PROJECTION_EXCLUDE_NOT_DESTINATION_ENTITY",
        ));
    let (_, report) = verify(ws.path());
    assert_eq!(
        report["report"]["coverage"]["excluded_entities"], 0,
        "{report}"
    );

    // The declaration, then the report names the entity as excluded with the
    // reason, in the data and in the markdown.
    memstead()
        .current_dir(ws.path())
        .args([
            "projection", "exclude", "notes/claims", "--entity-exclusions",
            r#"{"notes--the-retired-app-showed-diffs": "withdrawn 2026-08-18 with its subject; kept as the record of what was claimed"}"#,
            "--quiet",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("1 entity newly declared to carry no anchor"));
    let (_, report) = verify(ws.path());
    assert_eq!(
        report["report"]["coverage"]["unanchored_entities"],
        serde_json::json!([]),
        "{report}"
    );
    assert_eq!(report["report"]["coverage"]["excluded_entities"], 1);
    assert_eq!(
        report["report"]["excluded_entity_rationales"][0][0],
        "notes--the-retired-app-showed-diffs"
    );
    let md = memstead()
        .current_dir(ws.path())
        .args(["projection", "verify", "notes/claims", "--full", "--quiet"])
        .output()
        .unwrap();
    let md = String::from_utf8_lossy(&md.stdout);
    assert!(
        md.contains("entities without anchors (no verify can speak to them): 0; excluded on purpose (not owed): 1"),
        "{md}"
    );
    assert!(
        md.contains(
            "`notes--the-retired-app-showed-diffs` — withdrawn 2026-08-18 with its subject"
        ),
        "{md}"
    );
}
