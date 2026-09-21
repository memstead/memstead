//! `memstead mem fork`: the CLI face of `fork_mem`. One local fork
//! end to end (the JSON envelope with the ancestor and the base, the
//! `mem list` origin line in both shapes, the help text naming source,
//! sha, name and remote), the fork commit's effect as the user sees it
//! (`memstead anchors <fork-entity>` lists the source entity's rows,
//! `health --include anchors` counts the same rows on both, the
//! self-links name the fork), one typed refusal, and the folder-only
//! workspace refusal.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use memstead_git_branch::test_support::init_real_mem_repo_from_disk;
use serde_json::Value;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn write_mem_dir(root: &Path, name: &str) {
    let dir = root.join(name);
    let store = dir.join(".memstead");
    fs::create_dir_all(&store).unwrap();
    fs::write(
        store.join("config.json"),
        r#"{"schema": "default@1.0.0", "version": "0.2.0", "description": "the alpha mem"}"#,
    )
    .unwrap();
    fs::write(
        dir.join("alpha.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Alpha\n\n## Identity\n\nThe alpha entity.\n\n## Purpose\n\nSeed.\n",
    )
    .unwrap();
    // Gamma qualifies its self-links with the mem's own name, in both
    // spellings, beside a link into the other mem and a code span: what
    // the fork commit retargets, and what it leaves alone.
    fs::write(
        dir.join("gamma.md"),
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n\
             # Gamma\n\n## Identity\n\nBuilds on [[{name}--alpha]] and [[{name}:alpha|the alpha]]; \
             see [[{other}:alpha]]; the literal `[[{name}--alpha]]` stays.\n\n## Purpose\n\nSeed.\n",
            other = if name == "alpha" { "beta" } else { "alpha" }
        ),
    )
    .unwrap();
}

/// A mem-repo workspace with two mems, `alpha` and `beta`.
fn seed() -> TempDir {
    let ws = TempDir::new().unwrap();
    for m in ["alpha", "beta"] {
        write_mem_dir(ws.path(), m);
    }
    let dirs: Vec<_> = ["alpha", "beta"]
        .iter()
        .map(|m| (ws.path().join(m), *m))
        .collect();
    let refs: Vec<(&Path, &str)> = dirs.iter().map(|(p, m)| (p.as_path(), *m)).collect();
    init_real_mem_repo_from_disk(ws.path(), &refs);
    // The cross-mem links in the seed read conformant under a grant
    // each way, so the fixture carries no finding of its own.
    for (from, to) in [("alpha", "beta"), ("beta", "alpha")] {
        memstead()
            .current_dir(ws.path())
            .args(["workspace", "grant-cross-link", from, to, "--quiet"])
            .assert()
            .success();
    }
    // Delta in `alpha`: a url span row and a derived entity-grain row
    // whose artifact names a same-mem entity beside a foreign input.
    memstead()
        .current_dir(ws.path())
        .args([
            "create",
            "--mem",
            "alpha",
            "--title",
            "Delta",
            "--type",
            "spec",
            "--section",
            "identity=Delta quotes a page.",
            "--section",
            "purpose=Seed.",
            "--anchor",
            r#"{"artifact": "https://example.org/page", "grain": "url", "class": "anchored", "span": "the quoted words"}"#,
            "--anchor",
            r#"{"artifact": "alpha--alpha", "grain": "entity", "class": "derived", "derived_from": ["alpha--alpha", "beta--alpha"]}"#,
            "--quiet",
        ])
        .assert()
        .success();
    ws
}

/// `memstead anchors <id> --json`, the rows without their `entity_id`
/// (the one field the fork's rows must differ in).
fn anchor_rows(ws: &Path, id: &str) -> Value {
    let out = memstead()
        .current_dir(ws)
        .args(["anchors", id, "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "anchors {id}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut rows = json_stdout(&out)["anchors"].clone();
    for row in rows.as_array_mut().expect("rows") {
        assert_eq!(row["entity_id"], id, "{row}");
        row.as_object_mut().unwrap().remove("entity_id");
    }
    rows
}

/// The `anchors` axis of `health --mem <mem> --include anchors --json`
/// for one mem, minus the mem's own name, so two mems compare as equal
/// when they count the same rows.
fn health_anchors_axis(ws: &Path, mem: &str) -> Value {
    let out = memstead()
        .current_dir(ws)
        .args([
            "health",
            "--mem",
            mem,
            "--include",
            "anchors",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "health {mem}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = json_stdout(&out);
    let mut axis = report["anchors"][mem].clone();
    assert!(axis.is_object(), "the anchors axis for {mem}: {report}");
    axis.as_object_mut().unwrap().remove("mem");
    axis
}

fn json_stdout(out: &std::process::Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("one JSON document on stdout; got:\n{text}\n({e})"))
}

/// The local fork through the binary: the envelope names the fork,
/// its ancestor and its schema; `mem list` shows the origin in JSON
/// and in markdown, and shows none for the source.
#[test]
fn mem_fork_creates_the_fork_and_mem_list_shows_its_origin() {
    let ws = seed();
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "mem",
            "fork",
            "alpha",
            "alpha-fork",
            "--operator-mode",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let envelope = json_stdout(&out);
    assert_eq!(envelope["name"], "alpha-fork");
    assert_eq!(envelope["branch_ref"], "refs/heads/alpha-fork");
    assert_eq!(envelope["schema_ref"], "default@1.0.0");
    assert_eq!(envelope["forked_from"]["mem"], "alpha");
    let sha = envelope["forked_from"]["sha"].as_str().unwrap();
    assert_eq!(sha.len(), 40, "{sha}");
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "{sha}");
    // The base: the fork commit, the fork's own sha above the ancestor.
    let base = envelope["forked_from"]["base"].as_str().unwrap();
    assert_eq!(base.len(), 40, "{base}");
    assert!(base.chars().all(|c| c.is_ascii_hexdigit()), "{base}");
    assert_ne!(base, sha, "the base is not the ancestor");
    assert!(envelope["forked_from"]["remote"].is_null());
    assert_eq!(envelope["inherited_grants"], serde_json::json!(["beta"]));
    assert_eq!(envelope["warnings"], serde_json::json!([]));

    // The fork's entity lists the rows the source's entity has, under
    // the fork's id, with the same-mem artifact and input naming the
    // fork and resolving against the fork's entity; the source keeps
    // its own.
    let source_rows = anchor_rows(ws.path(), "alpha--delta");
    assert_eq!(
        source_rows.as_array().map(Vec::len),
        Some(2),
        "{source_rows}"
    );
    let expected_fork_rows: Value = serde_json::from_str(
        &serde_json::to_string(&source_rows)
            .unwrap()
            .replace("\"alpha--alpha\"", "\"alpha-fork--alpha\""),
    )
    .unwrap();
    assert_ne!(
        expected_fork_rows, source_rows,
        "the fixture has a same-mem target"
    );
    let fork_rows = anchor_rows(ws.path(), "alpha-fork--delta");
    assert_eq!(fork_rows, expected_fork_rows);
    let derived = fork_rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["grain"] == "entity")
        .unwrap();
    assert_eq!(derived["artifact"], "alpha-fork--alpha");
    assert_eq!(
        derived["derived_from"],
        serde_json::json!(["alpha-fork--alpha", "beta--alpha"])
    );
    assert_eq!(derived["state"], "resolves", "{derived}");
    assert!(
        source_rows
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["span"] == "the quoted words"),
        "{source_rows}"
    );
    // The fork counts what the source counts.
    let source_axis = health_anchors_axis(ws.path(), "alpha");
    assert_eq!(health_anchors_axis(ws.path(), "alpha-fork"), source_axis);
    assert!(
        source_axis["population"].as_u64().is_some_and(|n| n > 0)
            || source_axis["unobserved"].as_u64().is_some_and(|n| n > 0),
        "the axis counts rows: {source_axis}"
    );
    // The self-links name the fork; the other mem's link and the code
    // span are untouched.
    let out = memstead()
        .current_dir(ws.path())
        .args(["entity", "alpha-fork--gamma", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let gamma = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(gamma.contains("[[alpha-fork--alpha]]"), "{gamma}");
    assert!(gamma.contains("[[alpha-fork:alpha|the alpha]]"), "{gamma}");
    assert!(gamma.contains("[[beta:alpha]]"), "{gamma}");
    assert!(gamma.contains("`[[alpha--alpha]]`"), "{gamma}");
    assert_eq!(gamma.matches("[[alpha--alpha]]").count(), 1, "{gamma}");

    // The roster: the fork carries its origin, the source none.
    let out = memstead()
        .current_dir(ws.path())
        .args(["mem", "list", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let roster = json_stdout(&out);
    let rows = roster["mems"].as_array().unwrap();
    let fork = rows
        .iter()
        .find(|r| r["name"] == "alpha-fork")
        .expect("the fork is listed");
    assert_eq!(fork["forked_from"]["mem"], "alpha");
    assert_eq!(fork["forked_from"]["sha"], sha);
    assert_eq!(fork["forked_from"]["base"], base);
    assert!(fork["forked_from"]["remote"].is_null());
    assert_eq!(fork["description"], "the alpha mem");
    assert_eq!(fork["version"], "0.2.0");
    assert_eq!(fork["entity_count"], 3);
    let source = rows.iter().find(|r| r["name"] == "alpha").unwrap();
    assert!(source["forked_from"].is_null());

    let out = memstead()
        .current_dir(ws.path())
        .args(["mem", "list", "--quiet"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.contains("`alpha-fork`"))
        .unwrap_or_else(|| panic!("the fork's line: {text}"));
    assert!(
        line.contains(&format!("forked from `alpha`@{}", &sha[..12])),
        "{line}"
    );
    assert!(line.contains(&format!(", base {}", &base[..12])), "{line}");
    let source_line = text.lines().find(|l| l.contains("`alpha`")).unwrap();
    assert!(!source_line.contains("forked from"), "{source_line}");
}

/// A typed refusal rides the JSON envelope with its code; the
/// existing mem is untouched.
#[test]
fn mem_fork_refusal_is_typed() {
    let ws = seed();
    let out = memstead()
        .current_dir(ws.path())
        .args([
            "mem",
            "fork",
            "alpha",
            "beta",
            "--operator-mode",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let envelope = json_stdout(&out);
    assert_eq!(envelope["code"], "MEM_NAME_COLLISION");

    // A malformed `<source>@<sha>` is the CLI's own refusal.
    let out = memstead()
        .current_dir(ws.path())
        .args(["mem", "fork", "alpha@", "gamma", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(json_stdout(&out)["code"], "INVALID_INPUT");
}

/// A folder-only workspace has no branch to fork: `INVALID_INPUT`
/// naming the reason, from the engine.
#[test]
fn mem_fork_refuses_a_folder_only_workspace() {
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
    let out = memstead()
        .current_dir(ws.path())
        .args(["mem", "fork", "notes", "notes-fork", "--quiet", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let envelope = json_stdout(&out);
    assert_eq!(envelope["code"], "INVALID_INPUT", "{envelope}");
    let message = envelope["message"].as_str().unwrap_or_default();
    assert!(message.contains("mem-repo"), "{message}");
}

/// The help text names source, sha, name and remote.
#[test]
fn mem_fork_help_names_source_sha_name_and_remote() {
    let out = memstead().args(["mem", "fork", "--help"]).output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for needle in ["<SOURCE>", "<sha>", "<NAME>", "--remote <REMOTE>"] {
        assert!(help.contains(needle), "help names {needle}: {help}");
    }
}
