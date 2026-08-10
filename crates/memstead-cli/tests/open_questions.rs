#![cfg(feature = "mem-repo")]
//! The `open_questions` health axis (agent-trust plan 11): a composed
//! per-mem worklist of what the holding does not know — stubs,
//! recheck/unresolvable anchors, unsatisfied constraints, dangling
//! links, and a paired process mem's entries with negative findings
//! under the distinct already-searched heading. Composition only:
//! every count is asserted AGAINST the per-signal axis in the same
//! response, never against fixture constants — the no-disagreement
//! guarantee taken literally.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn run_ok(root: &Path, args: &[&str]) -> Vec<u8> {
    memstead()
        .current_dir(root)
        .env("MEMSTEAD_OPERATOR_MODE", "1")
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone()
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|e| {
        panic!(
            "--json output must parse: {e}\n--- stdout ---\n{}\n--- end ---",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// A schema package declaring a warn-tier `requires_when` constraint,
/// for the unsatisfied-constraint signal. No exemplar — also the
/// third-party-shape control.
fn write_constraint_schema(dir: &Path) {
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(
        dir.join("schema.yaml"),
        r#"name: qcheck
version: 0.1.0
description: constraint fixture
when_to_use: tests
types:
  - item
relationships:
  mode: strict
  definitions:
    - name: PART_OF
      description: hier
      default_weight: 3.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
    )
    .unwrap();
    fs::write(
        dir.join("types").join("item.yaml"),
        r#"name: item
description: t
when_to_use: tests
sections:
  - key: body
    heading: Body
    required: true
    search_weight: 10.0
    catch_all: true
    write_rules: []
metadata_fields:
  - key: status
    description: state
    field_type: string
    enum_values: [open, checked]
  - key: checked_by
    description: who checked
    field_type: string
title_weight: 100.0
text_fields:
  - body
hierarchy_relationship: PART_OF
no_self_loop_relationships: []
updatable_fields:
  - title
  - body
health_required_fields:
  - body
staleness_threshold_days: 90
constraints:
  - kind: requires_when
    field: checked_by
    when_field: status
    when_value: checked
write_rules: []
"#,
    )
    .unwrap();
}

/// Build the five-signal + process-pairing workspace:
/// - `hold` (default schema): an anchored entity spanning all four
///   anchor states, and a body wiki-link to an absent target (one
///   stub + one dangling link).
/// - `qmem` (qcheck schema): one entity violating the requires_when
///   constraint.
/// - binding `hold/proc` (destination `hold`) with a MOUNTED process
///   mem `proc` (ingest schema) holding one coverage_gap and one
///   negative_finding; binding `hold/ghostproc` with NO mounted mem.
fn build_workspace(root: &Path) {
    run_ok(root, &["mem-repo", "init", "."]);
    run_ok(root, &["mem", "init", "hold", "--no-gitignore"]);

    // Anchored entity across the four states.
    run_ok(
        root,
        &[
            "create",
            "--mem",
            "hold",
            "--title",
            "Holder",
            "--type",
            "spec",
            "--section",
            "identity=Anchored fixture entity.",
            "--section",
            "purpose=Verification states.",
        ],
    );
    for (name, content) in [
        ("src-a.txt", "alpha"),
        ("src-b.txt", "beta"),
        ("src-c.txt", "gamma"),
        ("src-d.txt", "delta"),
    ] {
        fs::write(root.join(name), content).unwrap();
    }
    let h = |content: &str| memstead_base::anchor::prepared_content_hash(content.as_bytes());
    let anchors = [
        format!(
            r#"{{"artifact":"src-a.txt","grain":"file","class":"anchored","hash":"{}","hash_stability":"stable"}}"#,
            h("alpha")
        ),
        format!(
            r#"{{"artifact":"src-b.txt","grain":"file","class":"anchored","hash":"{}","hash_stability":"stable"}}"#,
            h("beta")
        ),
        format!(
            r#"{{"artifact":"src-c.txt","grain":"file","class":"anchored","hash":"{}","hash_stability":"unstable"}}"#,
            h("gamma")
        ),
        format!(
            r#"{{"artifact":"src-d.txt","grain":"file","class":"anchored","hash":"{}","hash_stability":"stable"}}"#,
            h("delta")
        ),
    ];
    let mut args: Vec<String> = vec![
        "update".into(),
        "hold--holder".into(),
        "--auto-hash".into(),
        "--append".into(),
        "purpose= Anchored. See [[hold--ghost-target]].".into(),
    ];
    for a in &anchors {
        args.push("--anchor".into());
        args.push(a.clone());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_ok(root, &arg_refs);
    fs::write(root.join("src-b.txt"), "beta CHANGED").unwrap();
    fs::write(root.join("src-c.txt"), "gamma CHANGED").unwrap();
    fs::remove_file(root.join("src-d.txt")).unwrap();

    // Constraint-violating entity in its own mem.
    let pkg = root.join("qcheck-pkg");
    write_constraint_schema(&pkg);
    run_ok(root, &["schema", "install", pkg.to_str().unwrap()]);
    run_ok(
        root,
        &["mem", "init", "qmem", "--schema", "qcheck@0.1.0", "--no-gitignore"],
    );
    run_ok(
        root,
        &[
            "create",
            "--mem",
            "qmem",
            "--title",
            "Checked Without Checker",
            "--type",
            "item",
            "--section",
            "body=Violates requires_when.",
            "--metadata",
            "status=checked",
        ],
    );

    // Process pairing: binding hold/proc + mounted process mem `proc`.
    run_ok(
        root,
        &[
            "projection",
            "init",
            "--mem",
            "hold",
            "--source",
            "./src-a.txt",
            "--medium-type",
            "filesystem",
            "--name",
            "proc",
        ],
    );
    run_ok(
        root,
        &[
            "mem",
            "init",
            "proc",
            "--schema",
            "ingest@0.5.0",
            "--no-gitignore",
        ],
    );
    run_ok(
        root,
        &[
            "create",
            "--mem",
            "proc",
            "--title",
            "Uncovered corner",
            "--type",
            "coverage_gap",
            "--section",
            "area=A corner the destination lacks.",
            "--section",
            "evidence=src-a.txt paragraph two.",
        ],
    );
    run_ok(
        root,
        &[
            "create",
            "--mem",
            "proc",
            "--title",
            "No licensing note anywhere",
            "--type",
            "negative_finding",
            "--section",
            "sought=A licensing statement in the source.",
            "--section",
            "search_path=Full read of src-a.txt; grep for license.",
            "--section",
            "finding=Nothing — the source carries no licensing note.",
        ],
    );
    // Second binding with NO mounted process mem.
    run_ok(
        root,
        &[
            "projection",
            "init",
            "--mem",
            "hold",
            "--source",
            "./src-b.txt",
            "--medium-type",
            "filesystem",
            "--name",
            "ghostproc",
        ],
    );
}

#[test]
fn axis_composes_all_signals_and_matches_per_signal_axes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    build_workspace(root);

    let out = json(&run_ok(
        root,
        &[
            "--json",
            "health",
            "--include",
            "open_questions,stubs,anchors,constraints,dangling_links",
        ],
    ));
    let axis = &out["open_questions"];
    assert_eq!(axis["_item_cap"], 20, "cap is stated: {axis}");

    // --- hold: stubs — count equals the stubs axis' hold entries.
    let hold = &axis["hold"];
    let stubs_axis_hold = out["stubs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["id"].as_str().unwrap().starts_with("hold--"))
        .count() as u64;
    assert!(stubs_axis_hold >= 1, "fixture seeds a stub: {out}");
    assert_eq!(
        hold["stubs"]["count"].as_u64().unwrap(),
        stubs_axis_hold,
        "no-disagreement: stubs — {axis}"
    );
    assert_eq!(hold["stubs"]["items"][0]["kind"], "stub");

    // --- hold: anchors — counts equal the anchors axis' states.
    let anchors_hold = &out["anchors"]["hold"];
    assert_eq!(
        hold["anchors_recheck"]["count"], anchors_hold["recheck"],
        "no-disagreement: recheck — {axis} vs {anchors_hold}"
    );
    assert_eq!(
        hold["anchors_unresolvable"]["count"], anchors_hold["unresolvable"],
        "no-disagreement: unresolvable"
    );
    assert!(hold["anchors_recheck"]["count"].as_u64().unwrap() >= 1);
    assert!(hold["anchors_unresolvable"]["count"].as_u64().unwrap() >= 1);
    assert_eq!(hold["anchors_recheck"]["items"][0]["kind"], "anchor_recheck");
    assert!(
        hold["anchors_recheck"]["items"][0]["id"]
            .as_str()
            .unwrap()
            .starts_with("hold--"),
        "items carry the hanging id"
    );

    // --- hold: dangling links — count equals the dangling_links axis.
    let dangling_axis_hold = out["dangling_links"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["from"].as_str().unwrap().starts_with("hold--"))
        .count() as u64;
    assert!(dangling_axis_hold >= 1, "fixture seeds a dangling link");
    assert_eq!(
        hold["dangling_links"]["count"].as_u64().unwrap(),
        dangling_axis_hold,
        "no-disagreement: dangling links"
    );

    // --- qmem: unsatisfied constraints — count equals the
    // constraints axis' qmem entries.
    let qmem = &axis["qmem"];
    let constraints_axis_qmem = out["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["mem"] == "qmem")
        .count() as u64;
    assert!(constraints_axis_qmem >= 1, "fixture seeds a violation: {out}");
    assert_eq!(
        qmem["unsatisfied_constraints"]["count"].as_u64().unwrap(),
        constraints_axis_qmem,
        "no-disagreement: constraints"
    );
    assert_eq!(
        qmem["unsatisfied_constraints"]["items"][0]["kind"],
        "unsatisfied_constraint"
    );

    // --- process pairing (criterion 2): the mounted process mem's
    // entries appear, negative finding under the DISTINCT heading;
    // the unmounted binding is stated unresolvable — never an error.
    let process = hold["process"].as_array().expect("process section");
    let proc = process
        .iter()
        .find(|p| p["binding"] == "proc")
        .expect("mounted binding present");
    assert_eq!(proc["resolvable"], true);
    let open = serde_json::to_string(&proc["open_entries"]).unwrap();
    assert!(
        open.contains("coverage_gap") && open.contains("uncovered-corner"),
        "coverage gap under open work: {open}"
    );
    assert!(
        !open.contains("negative_finding"),
        "negative findings must NOT be in the todo pile: {open}"
    );
    let searched = serde_json::to_string(&proc["already_searched"]).unwrap();
    assert!(
        searched.contains("negative_finding") && searched.contains("no-licensing-note"),
        "negative finding under already_searched: {searched}"
    );
    let ghost = process
        .iter()
        .find(|p| p["binding"] == "ghostproc")
        .expect("unmounted binding stated");
    assert_eq!(ghost["resolvable"], false, "{ghost}");
    assert!(ghost.get("open_entries").is_none());

    // --- a hole-free mem reports an empty axis entry, not an error.
    let proc_entry = &axis["proc"];
    assert_eq!(proc_entry["stubs"]["count"], 0);
    assert_eq!(proc_entry["dangling_links"]["count"], 0);

    // --- without the include: no axis key at all.
    let plain = json(&run_ok(root, &["--json", "health"]));
    assert!(
        plain.get("open_questions").is_none(),
        "health without the include must not carry the axis"
    );
}

/// The item cap triggers with an explicit remainder count — silent
/// truncation is the anti-pattern.
#[test]
fn item_cap_truncates_with_explicit_remainder() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    run_ok(root, &["mem-repo", "init", "."]);
    run_ok(root, &["mem", "init", "hold", "--no-gitignore"]);

    // One entity whose body wiki-links 25 absent targets → 25 stubs.
    let links: String = (0..25)
        .map(|i| format!("[[hold--ghost-{i}]]"))
        .collect::<Vec<_>>()
        .join(" ");
    run_ok(
        root,
        &[
            "create",
            "--mem",
            "hold",
            "--title",
            "Linker",
            "--type",
            "spec",
            "--section",
            &format!("identity=Links: {links}"),
            "--section",
            "purpose=Cap fixture.",
        ],
    );

    let out = json(&run_ok(
        root,
        &["--json", "health", "--include", "open_questions"],
    ));
    let stubs = &out["open_questions"]["hold"]["stubs"];
    assert_eq!(stubs["count"], 25, "{out}");
    assert_eq!(stubs["items"].as_array().unwrap().len(), 20, "capped at 20");
    assert_eq!(stubs["more"], 5, "explicit remainder");
}
