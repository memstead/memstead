//! The checks-axis independence reading compares a check's identity with
//! every identity that mutated the verified plan, its criteria or its
//! session-log notes since the criterion was written — never with the
//! criterion's author alone — and the `transition_requires_checks` gate
//! consumes that reading, so a plan cannot complete on the executor's own
//! checks.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

/// A folder workspace with one mem `plans` pinned to a gated schema:
/// `plan` completes only when every incoming VERIFIES criterion carries a
/// fresh, independent ok check; `criterion` and `note` (hierarchy PART_OF)
/// are plain typed entities.
fn seed() -> TempDir {
    seed_with(PLAN_GATE)
}

const PLAN_GATE: &str = "constraints:\n  - kind: transition_requires_checks\n    field: status\n    to_value: complete\n    relationships: [VERIFIES]\n    direction: incoming\n    severity: block\n";

const SELF_GATE: &str = "constraints:\n  - kind: transition_requires_self_check\n    field: status\n    to_value: complete\n    check_kind: x-approval\n    severity: block\n";

fn seed_with(plan_gate: &str) -> TempDir {
    seed_on(plan_gate, false)
}

/// The fixture on a folder mem (`init`) or a git-branch mem (`mem-repo
/// init`, then `mem init`).
fn seed_on(plan_gate: &str, git_branch: bool) -> TempDir {
    let ws = TempDir::new().unwrap();
    if git_branch {
        for args in [
            &["mem-repo", "init", ".", "--no-gitignore", "--quiet"][..],
            &[
                "workspace",
                "allow-create",
                "--schema",
                "*",
                "p*",
                "--quiet",
            ][..],
            &[
                "mem",
                "init",
                "plans",
                "--schema",
                "default@1.3.0",
                "--quiet",
            ][..],
        ] {
            memstead()
                .current_dir(ws.path())
                .args(args)
                .assert()
                .success();
        }
    } else {
        memstead()
            .current_dir(ws.path())
            .args([
                "init",
                "--name",
                "plans",
                "--schema",
                "default@1.3.0",
                "--quiet",
            ])
            .assert()
            .success();
    }
    let dir = ws
        .path()
        .join(".memstead")
        .join("schemas")
        .join("gated@0.1.0");
    fs::create_dir_all(dir.join("types")).unwrap();
    fs::write(
        dir.join("schema.yaml"),
        r#"name: gated
version: 0.1.0
description: transition_requires_checks test schema
when_to_use: tests
types:
  - plan
  - criterion
  - note
relationships:
  mode: strict
  definitions:
    - name: VERIFIES
      description: v
      default_weight: 3.0
      acyclic: true
    - name: PART_OF
      description: hier
      default_weight: 1.0
      acyclic: true
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#,
    )
    .unwrap();
    let body = |name: &str, extra_meta: &str, constraints: &str| {
        format!(
            "name: {name}\ndescription: t\nwhen_to_use: Here\nsections:\n  - key: body\n    heading: Body\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:{extra_meta}\ntitle_weight: 100.0\ntext_fields:\n  - body\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: []\nupdatable_fields:\n  - title\n  - body\nhealth_required_fields:\n  - body\nstaleness_threshold_days: 90\nwrite_rules: []\n{constraints}"
        )
    };
    fs::write(
        dir.join("types").join("plan.yaml"),
        body(
            "plan",
            "\n  - key: status\n    description: s\n    field_type: string\n    default_value: draft\n    enum_values: [draft, complete]",
            plan_gate,
        ),
    )
    .unwrap();
    fs::write(
        dir.join("types").join("criterion.yaml"),
        body("criterion", " []", ""),
    )
    .unwrap();
    fs::write(
        dir.join("types").join("note.yaml"),
        body(
            "note",
            "\n  - key: status\n    description: s\n    field_type: string\n    required: false\n    enum_values: [draft, complete]",
            "",
        ),
    )
    .unwrap();
    memstead()
        .current_dir(ws.path())
        .args(["mem", "set-schema", "plans", "gated@0.1.0", "--quiet"])
        .assert()
        .success();
    ws
}

fn create(ws: &Path, identity: &str, ty: &str, title: &str, relation: Option<&str>) {
    let mut args = vec![
        "create",
        "--quiet",
        "--identity",
        identity,
        "--role",
        "author",
        "--type",
        ty,
        "--title",
        title,
        "--section",
        "body=x",
    ];
    if let Some(r) = relation {
        args.push("--relation");
        args.push(r);
    }
    memstead().current_dir(ws).args(&args).assert().success();
}

fn check(ws: &Path, identity: Option<&str>, entity: &str) {
    let mut args = vec![
        "check",
        entity,
        "--verdict",
        "ok",
        "--role",
        "checker",
        "--method",
        "test",
        "--quiet",
    ];
    if let Some(i) = identity {
        args.push("--identity");
        args.push(i);
    }
    memstead().current_dir(ws).args(&args).assert().success();
}

fn independence(ws: &Path) -> serde_json::Value {
    let out = memstead()
        .current_dir(ws)
        .args(["health", "--include", "checks", "--json", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["checks"]["plans"]["independence"].clone()
}

fn complete(ws: &Path, identity: &str) -> std::process::Output {
    memstead()
        .current_dir(ws)
        .args([
            "update",
            "plans--the-plan",
            "--metadata",
            "status=complete",
            "--auto-hash",
            "--identity",
            identity,
            "--role",
            "author",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap()
}

/// Identity A authors the plan and the criterion; identity B
/// mutates the plan (a note) and checks: `self_checked`, the transition
/// refuses; identity C, which mutated nothing, checks: the criterion reads
/// `confirmed_independent` and the transition succeeds. Refusal
/// complement: a check without an identity stays `unconfirmable` and
/// refuses.
#[test]
fn executors_own_checks_read_self_checked_and_do_not_close_the_gate() {
    let ws = seed();
    let root = ws.path();
    create(root, "identity-a", "plan", "The Plan", None);
    create(
        root,
        "identity-a",
        "criterion",
        "The Criterion",
        Some("VERIFIES:plans--the-plan"),
    );
    // B executes: a session-log note under the plan.
    create(
        root,
        "identity-b",
        "note",
        "Session One",
        Some("PART_OF:plans--the-plan"),
    );

    // Refusal complement first: no identity, no promotion.
    check(root, None, "plans--the-criterion");
    let ind = independence(root);
    assert_eq!(
        ind["unconfirmable"]["items"],
        serde_json::json!(["plans--the-criterion"]),
        "{ind}"
    );
    let out = complete(root, "identity-b");
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(env["code"], "CONSTRAINT_UNSATISFIED", "{env}");
    assert!(env.to_string().contains("unconfirmable"), "{env}");

    // The executor's own check: self_checked, and the gate stays shut.
    check(root, Some("identity-b"), "plans--the-criterion");
    let ind = independence(root);
    assert_eq!(
        ind["self_checked"]["items"],
        serde_json::json!(["plans--the-criterion"]),
        "{ind}"
    );
    let executors = ind["executors"]["plans--the-criterion"].as_array().unwrap();
    let names: Vec<&str> = executors.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        names.contains(&"identity-a") && names.contains(&"identity-b"),
        "{ind}"
    );
    let out = complete(root, "identity-b");
    assert!(!out.status.success());
    let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(env["code"], "CONSTRAINT_UNSATISFIED", "{env}");
    assert!(env.to_string().contains("self_checked"), "{env}");

    // An identity that mutated nothing since the criterion was written.
    check(root, Some("identity-c"), "plans--the-criterion");
    let ind = independence(root);
    assert_eq!(
        ind["confirmed_independent"]["items"],
        serde_json::json!(["plans--the-criterion"]),
        "{ind}"
    );
    let out = complete(root, "identity-b");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // The gates brief agrees: the plan is closed.
    let out = memstead()
        .current_dir(root)
        .args(["gates", "--mem", "plans", "--quiet"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Closed (at `complete`): `plans--the-plan`"),
        "{text}"
    );
}

/// First half: the ledger line shape is unchanged — a row written
/// before the plan (no new field) derives under the new comparator. The
/// row is appended by hand in the exact pre-plan shape.
#[test]
fn old_ledger_rows_derive_under_the_new_comparator() {
    let ws = seed();
    let root = ws.path();
    create(root, "identity-a", "plan", "The Plan", None);
    create(
        root,
        "identity-a",
        "criterion",
        "The Criterion",
        Some("VERIFIES:plans--the-plan"),
    );
    let out = memstead()
        .current_dir(root)
        .args(["entity", "plans--the-criterion", "--json", "--quiet"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let hash = v["_hash"].as_str().unwrap().to_string();
    let ledger = root
        .join(".memstead")
        .join("state")
        .join("checks")
        .join("checks.jsonl");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 60;
    // The pre-plan line shape: ts, entity, verdict, method, entity_hash,
    // actor, client, role, identity — nothing new.
    fs::write(
        &ledger,
        format!(
            "{{\"ts\":{ts},\"entity\":\"plans--the-criterion\",\"verdict\":\"ok\",\"method\":\"old row\",\"entity_hash\":\"{hash}\",\"actor\":\"cli\",\"client\":\"memstead-cli@0.15.0\",\"role\":\"checker\",\"identity\":\"identity-a\"}}\n"
        ),
    )
    .unwrap();
    let ind = independence(root);
    // identity-a authored the criterion: an executor, so self_checked
    // under the new comparator, and the old row parsed as-is.
    assert_eq!(
        ind["self_checked"]["items"],
        serde_json::json!(["plans--the-criterion"]),
        "{ind}"
    );
    assert!(
        ind["comparator"]
            .as_str()
            .unwrap()
            .contains("since the criterion was written")
    );
}

/// The checks gate after completion: a write that completes the plan and
/// changes its body in the same call commits (its checks sit on other
/// entities, nothing is carried). An edit of a criterion afterwards leaves
/// the criterion `check_stale` and the constraints axis lists the plan's
/// gate as a standing violation.
#[test]
fn a_criterion_edited_after_completion_reads_stale_and_the_gate_stands_violated() {
    let ws = seed();
    let root = ws.path();
    create(root, "identity-a", "plan", "The Plan", None);
    create(
        root,
        "identity-a",
        "criterion",
        "The Criterion",
        Some("VERIFIES:plans--the-plan"),
    );
    check(root, Some("identity-c"), "plans--the-criterion");
    memstead()
        .current_dir(root)
        .args([
            "update",
            "plans--the-plan",
            "--metadata",
            "status=complete",
            "--section",
            "body=rewritten in the completing write",
            "--auto-hash",
            "--identity",
            "identity-a",
            "--role",
            "author",
            "--quiet",
        ])
        .assert()
        .success();
    memstead()
        .current_dir(root)
        .args([
            "update",
            "plans--the-criterion",
            "--section",
            "body=edited after completion",
            "--auto-hash",
            "--identity",
            "identity-a",
            "--quiet",
        ])
        .assert()
        .success();
    let out = memstead()
        .current_dir(root)
        .args([
            "health",
            "--include",
            "checks,constraints",
            "--json",
            "--quiet",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["checks"]["plans"]["check_stale"], 1, "{v}");
    let text = v["constraints"].to_string();
    assert!(
        text.contains("plans--the-plan") && text.contains("transition_requires_checks"),
        "{text}"
    );
}

fn cli(ws: &Path, args: &[&str]) -> std::process::Output {
    memstead().current_dir(ws).args(args).output().unwrap()
}

fn edit(ws: &Path, identity: Option<&str>, args: &[&str]) {
    let mut all = vec!["update", "plans--the-plan", "--auto-hash", "--quiet"];
    all.extend_from_slice(args);
    if let Some(i) = identity {
        all.push("--identity");
        all.push(i);
    }
    let out = cli(ws, &all);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn approve(ws: &Path, identity: &str) {
    let out = cli(
        ws,
        &[
            "check",
            "plans--the-plan",
            "--verdict",
            "ok",
            "--kind",
            "x-approval",
            "--role",
            "checker",
            "--identity",
            identity,
            "--quiet",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The plan's self-check standing as the entity read derives it, and the
/// first author the provenance read names.
fn standing(ws: &Path) -> (String, String) {
    let out = cli(
        ws,
        &[
            "entity",
            "plans--the-plan",
            "--provenance",
            "--json",
            "--quiet",
        ],
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let prov = &v["mutation_provenance"];
    let created_by = prov["created_by"]["identity"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let health = cli(
        ws,
        &["health", "--include", "constraints", "--json", "--quiet"],
    );
    (
        String::from_utf8_lossy(&health.stdout).to_string(),
        created_by,
    )
}

/// Independence on a git-branch mem and on a folder mem: A
/// creates, B rewrites one second later and in the same second, B
/// checks, C sets the gated value: refused naming `self_checked`; the
/// first author reads A. D, who wrote nothing, checks and completes:
/// the gate opens. Before the repair the one-second case opened the
/// gate on B's check (the comparator was the first author only).
#[test]
fn independence_counts_every_writer_on_both_backends() {
    for (pause, git_branch) in [(true, true), (false, true), (true, false), (false, false)] {
        let ws = seed_on(SELF_GATE, git_branch);
        if git_branch {
            assert!(ws.path().join("mem-repo").exists(), "a git-branch mem");
        }
        let root = ws.path();
        create(root, "identity-a", "plan", "The Plan", None);
        if pause {
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        edit(
            root,
            Some("identity-b"),
            &["--section", "body=rewritten by b"],
        );
        approve(root, "identity-b");
        let out = cli(
            root,
            &[
                "update",
                "plans--the-plan",
                "--metadata",
                "status=complete",
                "--auto-hash",
                "--identity",
                "identity-c",
                "--json",
                "--quiet",
            ],
        );
        assert!(
            !out.status.success(),
            "pause {pause} git-branch {git_branch}"
        );
        let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(env["code"], "CONSTRAINT_UNSATISFIED", "{env}");
        assert!(env.to_string().contains("self_checked"), "{env}");
        let (_, created_by) = standing(root);
        assert_eq!(created_by, "identity-a", "pause {pause}");

        approve(root, "identity-d");
        edit(root, Some("identity-d"), &["--metadata", "status=complete"]);
        let (health, _) = standing(root);
        assert!(
            !health.contains("transition_requires_self_check"),
            "pause {pause}: {health}"
        );
    }
}

/// The checks column of the verb table on a git-branch mem: a create
/// that promotes a placeholder with confirmed incoming criteria lands the
/// gated value; the same create with an unconfirmed criterion refuses,
/// nothing written; a retype onto the gated type commits when the gate is
/// met and refuses when it is not.
#[test]
fn the_checks_column_of_the_verb_table() {
    let ws = seed();
    let root = ws.path();
    // The criterion names a plan that does not exist yet: a placeholder.
    create(
        root,
        "identity-a",
        "criterion",
        "The Criterion",
        Some("VERIFIES:plans--the-plan"),
    );
    let born = |status: &str| {
        cli(
            root,
            &[
                "create",
                "--type",
                "plan",
                "--title",
                "The Plan",
                "--section",
                "body=x",
                "--metadata",
                &format!("status={status}"),
                "--identity",
                "identity-a",
                "--json",
                "--quiet",
            ],
        )
    };
    let out = born("complete");
    assert!(!out.status.success(), "unconfirmed criterion");
    let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(env["code"], "CONSTRAINT_UNSATISFIED", "{env}");
    check(root, Some("identity-c"), "plans--the-criterion");
    let out = born("complete");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Retype a note at `complete` onto the gated plan type.
    create(root, "identity-a", "note", "The Note", None);
    edit_note(root, &["--metadata", "status=complete"]);
    create(
        root,
        "identity-a",
        "criterion",
        "Note Criterion",
        Some("VERIFIES:plans--the-note"),
    );
    let retype = || {
        cli(
            root,
            &[
                "retype",
                "plans--the-note",
                "--type",
                "plan",
                "--auto-hash",
                "--identity",
                "identity-a",
                "--json",
                "--quiet",
            ],
        )
    };
    let out = retype();
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    check(root, Some("identity-c"), "plans--note-criterion");
    let out = retype();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn edit_note(ws: &Path, args: &[&str]) {
    let mut all = vec!["update", "plans--the-note", "--auto-hash", "--quiet"];
    all.extend_from_slice(args);
    let out = cli(ws, &all);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// On a git-branch mem: a rename that rewrites a link inside an entry
/// counts the renamer as a writer of that entry, so the renamer's own
/// check of it does not open the self-check gate.
#[test]
fn a_rename_counts_its_identity_on_the_referrers_it_rewrites_on_git_branch() {
    let ws = seed_on(SELF_GATE, true);
    let root = ws.path();
    create(root, "identity-a", "plan", "The Target", None);
    let out = cli(
        root,
        &[
            "create",
            "--type",
            "plan",
            "--title",
            "The Plan",
            "--section",
            "body=see [[plans--the-target]].",
            "--relation",
            "PART_OF:plans--the-target",
            "--identity",
            "identity-a",
            "--quiet",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = cli(
        root,
        &[
            "rename",
            "plans--the-target",
            "The Moved",
            "--auto-hash",
            "--identity",
            "identity-c",
            "--quiet",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    approve(root, "identity-c");
    let out = cli(
        root,
        &[
            "update",
            "plans--the-plan",
            "--metadata",
            "status=complete",
            "--auto-hash",
            "--identity",
            "identity-e",
            "--json",
            "--quiet",
        ],
    );
    assert!(!out.status.success(), "the renamer wrote what it checked");
    let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(env.to_string().contains("self_checked"), "{env}");
}

/// A self-check transition write that changes a section only at its edge
/// (leading spaces turn a paragraph into a code block) changes content:
/// it refuses, the ledger unchanged, on both backends.
#[test]
fn an_edge_whitespace_change_beside_the_transition_refuses() {
    for git_branch in [false, true] {
        let ws = seed_on(SELF_GATE, git_branch);
        let root = ws.path();
        create(root, "identity-a", "plan", "The Plan", None);
        approve(root, "identity-c");
        let ledger = root.join(".memstead/state/checks/checks.jsonl");
        let before = fs::read_to_string(&ledger).unwrap();
        let out = cli(
            root,
            &[
                "update",
                "plans--the-plan",
                "--metadata",
                "status=complete",
                "--section",
                "body=    x",
                "--auto-hash",
                "--identity",
                "identity-b",
                "--json",
                "--quiet",
            ],
        );
        assert!(
            !out.status.success(),
            "git-branch {git_branch}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(env["code"], "CONSTRAINT_UNSATISFIED", "{env}");
        assert_eq!(fs::read_to_string(&ledger).unwrap(), before);
    }
}

/// A proposal merge lands an entry two fork identities wrote under the
/// last one's name; the earlier writer rides the merge commit as a
/// `Co-Writer:` and its own check of the merged text does not open the
/// self-check gate.
#[test]
fn a_fork_writer_whose_text_a_proposal_merge_lands_cannot_confirm_it() {
    let ws = seed_on(SELF_GATE, true);
    let root = ws.path();
    create(root, "identity-a", "plan", "The Plan", None);
    let ok = |args: &[&str]| {
        let out = cli(root, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        out
    };
    ok(&[
        "mem",
        "fork",
        "plans",
        "pfork",
        "--identity",
        "identity-a",
        "--quiet",
    ]);
    ok(&[
        "update",
        "pfork--the-plan",
        "--auto-hash",
        "--section",
        "body=x wrote this",
        "--identity",
        "identity-x",
        "--quiet",
    ]);
    ok(&[
        "update",
        "pfork--the-plan",
        "--auto-hash",
        "--section",
        "body=x wrote this.",
        "--identity",
        "identity-y",
        "--quiet",
    ]);
    let brief = ok(&["proposal", "brief", "pfork", "--json", "--quiet"]);
    let mut v: serde_json::Value = serde_json::from_slice(&brief.stdout).unwrap();
    v["dispositions"]["the-plan"]["disposition"] = "adopt".into();
    let file = root.join("dispositions.json");
    std::fs::write(&file, v.to_string()).unwrap();
    ok(&[
        "proposal",
        "merge",
        "pfork",
        "--dispositions",
        file.to_str().unwrap(),
        "--identity",
        "identity-m",
        "--quiet",
    ]);
    approve(root, "identity-x");
    let out = complete(root, "identity-e");
    assert!(
        !out.status.success(),
        "identity-x wrote the merged text and confirmed it: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn must(ws: &Path, args: &[&str]) -> std::process::Output {
    let out = cli(ws, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    out
}

/// Brief the fork, adopt `slug`, merge as identity-m.
fn merge_fork(ws: &Path, fork: &str, slug: &str) {
    let brief = must(ws, &["proposal", "brief", fork, "--json", "--quiet"]);
    let mut v: serde_json::Value = serde_json::from_slice(&brief.stdout).unwrap();
    v["dispositions"][slug]["disposition"] = "adopt".into();
    let file = ws.join(format!("{fork}-dispositions.json"));
    std::fs::write(&file, v.to_string()).unwrap();
    must(
        ws,
        &[
            "proposal",
            "merge",
            fork,
            "--dispositions",
            file.to_str().unwrap(),
            "--identity",
            "identity-m",
            "--quiet",
        ],
    );
}

/// A fork of a fork: the co-writer the inner merge recorded travels on
/// through the outer merge, so the inner fork's writer cannot confirm the
/// text it wrote after two merges.
#[test]
fn a_co_writer_survives_a_second_merge() {
    let ws = seed_on(SELF_GATE, true);
    let root = ws.path();
    create(root, "identity-a", "plan", "The Plan", None);
    must(
        root,
        &["workspace", "allow-create", "--schema", "*", "*", "--quiet"],
    );
    must(
        root,
        &[
            "mem",
            "fork",
            "plans",
            "pfork",
            "--identity",
            "identity-a",
            "--quiet",
        ],
    );
    must(
        root,
        &[
            "mem",
            "fork",
            "pfork",
            "qfork",
            "--identity",
            "identity-a",
            "--quiet",
        ],
    );
    for (body, who) in [
        ("x wrote this", "identity-x"),
        ("x wrote this.", "identity-y"),
    ] {
        must(
            root,
            &[
                "update",
                "qfork--the-plan",
                "--auto-hash",
                "--section",
                &format!("body={body}"),
                "--identity",
                who,
                "--quiet",
            ],
        );
    }
    merge_fork(root, "qfork", "the-plan");
    merge_fork(root, "pfork", "the-plan");
    approve(root, "identity-x");
    assert!(!complete(root, "identity-c").status.success());
}

/// A fork write without an identity lands through a merge as a write
/// without an identity: no outside check of the merged text confirms.
#[test]
fn an_anonymous_fork_write_a_merge_lands_reads_unconfirmable() {
    let ws = seed_on(SELF_GATE, true);
    let root = ws.path();
    create(root, "identity-a", "plan", "The Plan", None);
    must(
        root,
        &[
            "mem",
            "fork",
            "plans",
            "pfork",
            "--identity",
            "identity-a",
            "--quiet",
        ],
    );
    must(
        root,
        &[
            "update",
            "pfork--the-plan",
            "--auto-hash",
            "--section",
            "body=anon wrote this",
            "--quiet",
        ],
    );
    must(
        root,
        &[
            "update",
            "pfork--the-plan",
            "--auto-hash",
            "--section",
            "body=anon wrote this.",
            "--identity",
            "identity-x",
            "--quiet",
        ],
    );
    merge_fork(root, "pfork", "the-plan");
    approve(root, "identity-e");
    let out = complete(root, "identity-c");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("unconfirmable"));
}

fn upd(ws: &Path, id: &str, body: &str, who: Option<&str>) {
    let section = format!("body={body}");
    let mut args = vec![
        "update",
        id,
        "--auto-hash",
        "--section",
        section.as_str(),
        "--quiet",
    ];
    if let Some(w) = who {
        args.push("--identity");
        args.push(w);
    }
    must(ws, &args);
}

fn two_forks(ws: &Path) {
    create(ws, "identity-a", "plan", "The Plan", None);
    must(
        ws,
        &["workspace", "allow-create", "--schema", "*", "*", "--quiet"],
    );
    must(
        ws,
        &[
            "mem",
            "fork",
            "plans",
            "pfork",
            "--identity",
            "identity-a",
            "--quiet",
        ],
    );
    must(
        ws,
        &[
            "mem",
            "fork",
            "pfork",
            "qfork",
            "--identity",
            "identity-a",
            "--quiet",
        ],
    );
}

/// A rename on the middle fork between two merges: the inner fork's
/// co-writer, recorded under the entity's earlier id, still counts.
#[test]
fn a_co_writer_survives_a_rename_between_two_merges() {
    for checker in ["identity-x", "identity-w"] {
        let ws = seed_on(SELF_GATE, true);
        let root = ws.path();
        two_forks(root);
        upd(root, "qfork--the-plan", "x wrote", Some("identity-x"));
        upd(root, "qfork--the-plan", "y wrote", Some("identity-y"));
        merge_fork(root, "qfork", "the-plan");
        must(
            root,
            &[
                "rename",
                "pfork--the-plan",
                "Renamed Plan",
                "--auto-hash",
                "--identity",
                "identity-y",
                "--quiet",
            ],
        );
        let brief = must(root, &["proposal", "brief", "pfork", "--json", "--quiet"]);
        let mut v: serde_json::Value = serde_json::from_slice(&brief.stdout).unwrap();
        eprintln!(
            "summary: {} entries: {}",
            v["summary"],
            v["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| format!("{}:{}", e["slug"], e["status"]))
                .collect::<Vec<_>>()
                .join(",")
        );
        let slugs: Vec<String> = v["dispositions"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for s in &slugs {
            v["dispositions"][s]["disposition"] = "adopt".into();
        }
        let file = root.join("p.json");
        std::fs::write(&file, v.to_string()).unwrap();
        must(
            root,
            &[
                "proposal",
                "merge",
                "pfork",
                "--dispositions",
                file.to_str().unwrap(),
                "--identity",
                "identity-m",
                "--quiet",
            ],
        );
        let l = cli(root, &["list", "--mem", "plans", "--quiet"]);
        eprintln!("list: {}", String::from_utf8_lossy(&l.stdout));
        let id = "plans--renamed-plan";
        must(
            root,
            &[
                "check",
                id,
                "--verdict",
                "ok",
                "--kind",
                "x-approval",
                "--role",
                "checker",
                "--identity",
                checker,
                "--quiet",
            ],
        );
        let out = cli(
            root,
            &[
                "update",
                id,
                "--metadata",
                "status=complete",
                "--auto-hash",
                "--identity",
                "identity-c",
                "--quiet",
                "--json",
            ],
        );
        eprintln!(
            "[g4 {checker}] complete: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        if checker == "identity-w" {
            assert!(out.status.success());
        } else {
            assert!(
                !out.status.success(),
                "x wrote the renamed text and confirmed it"
            );
        }
    }
}
