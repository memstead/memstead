//! `projection exclude` resolves the artifact id and refuses an unknown one
//! (backlog-decisions plan B11): on a bound fixture whose source pointer is
//! a sub-tree (and a second whose pointer lies outside the workspace root,
//! the flagship's shape), the source-relative id and the workspace-relative
//! one resolve to the same canonical id, the ledger holds that one form, the
//! response lists what was recorded, the next verify counts the artifact as
//! disposed excluded, and an id resolving to no artifact refuses the whole
//! call naming the nearest known ids; a ledger written before the plan in
//! the canonical form keeps filtering unchanged.

use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn write_store(root: &Path, rel: &str, contents: &str) {
    let path = root.join(".memstead").join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A folder-mem workspace bound to a source at `pointer` (relative to the
/// workspace root) holding `docs/a.md` and `docs/b.md` under git; returns
/// the workspace root. The source dir is created at `root.join(pointer)`
/// normalised, so `..`-relative pointers land beside the workspace.
fn bound_workspace(tmp: &TempDir, pointer: &str) -> std::path::PathBuf {
    let root = tmp.path().join("ws");
    std::fs::create_dir_all(root.join("engine-mem").join(".memstead")).unwrap();
    std::fs::write(
        root.join("engine-mem")
            .join(".memstead")
            .join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    std::fs::write(
        root.join("engine-mem").join("seed.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Seed\n\n## Identity\n\nThe seed.\n\n## Purpose\n\nExists.\n",
    )
    .unwrap();
    write_store(
        &root,
        "workspace.toml",
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );
    write_store(
        &root,
        "state/mounts.json",
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"engine","schema":"default@1.0.0","storage":{"type":"folder","path":"engine-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    );
    write_store(
        &root,
        "projections/engine/docs.json",
        &format!(
            r#"{{"version":2,"intent":"model the docs","sources":[{{"name":"docs","type":"codebase","pointer":"{pointer}","change_detection":"git","scope":[{{"path":"**/*.md","mode":"allow"}}]}}],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{{"build":{{"mode":"discovery","trigger":"loop","batch_size":20}},"sync":{{"trigger":"manual","batch_size":20}},"verify":{{"trigger":"manual","batch_size":20,"adjudication_mode":"strict"}}}}}}"#
        ),
    );
    let src = root.join(pointer);
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs").join("a.md"), "# A\n").unwrap();
    std::fs::write(src.join("docs").join("b.md"), "# B\n").unwrap();
    git(&src, &["init", "-q"]);
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "-q", "-m", "init"]);
    root
}

fn exclude(root: &Path, payload: &str) -> (bool, Value) {
    let out = memstead()
        .current_dir(root)
        .args([
            "--json",
            "projection",
            "exclude",
            "engine/docs",
            "--exclusions",
            payload,
        ])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let json_start = text.find('{').unwrap_or(0);
    let v: Value = serde_json::from_str(text[json_start..].trim()).unwrap_or(Value::Null);
    (out.status.success(), v)
}

fn ledger(root: &Path) -> Value {
    let path = root
        .join(".memstead")
        .join("state")
        .join("advance")
        .join("engine")
        .join("docs.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn verify(root: &Path) -> Value {
    let out = memstead()
        .current_dir(root)
        .args(["--json", "projection", "verify", "engine/docs", "--full"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn exercise(pointer: &str, canonical_a: &str, canonical_b: &str) {
    let tmp = TempDir::new().unwrap();
    let root = bound_workspace(&tmp, pointer);

    // Before any exclusion: both artifacts are the uncovered backfill.
    let report = verify(&root);
    assert_eq!(report["report"]["disposed_excluded"], 0, "{report}");
    // C7 baseline: with no exclusion the coverage section is what it always
    // was — both artifacts in the array, no excluded count, no extra clause
    // on the figure. This is the plan's refusal complement, taken on the
    // same fixture the assertion runs on.
    let uncovered_before = report["report"]["coverage"]["uncovered"].to_string();
    assert!(
        uncovered_before.contains(canonical_a) && uncovered_before.contains(canonical_b),
        "{report}"
    );
    assert_eq!(report["report"]["coverage"]["excluded"], 0, "{report}");
    let md_before = report["report_markdown"].as_str().unwrap();
    assert!(
        md_before.contains("- uncovered (no anchor): 2\n"),
        "the bare figure, no excluded clause: {md_before}"
    );
    assert!(!md_before.contains("excluded on purpose"), "{md_before}");

    // The source-relative form resolves to the canonical id.
    let (ok, v) = exclude(&root, r#"{"docs/a.md": "probe"}"#);
    assert!(ok, "{v}");
    assert_eq!(v["added"], 1);
    assert_eq!(v["recorded"][0]["requested"], "docs/a.md");
    assert_eq!(v["recorded"][0]["canonical"], canonical_a);
    let stored = ledger(&root);
    assert!(stored["exclusions"][canonical_a].is_string(), "{stored}");
    assert!(
        stored["exclusions"]["docs/a.md"].is_null(),
        "no second spelling: {stored}"
    );

    // The workspace-relative form gives the same result, byte for byte.
    let (ok, v2) = exclude(&root, &format!(r#"{{"{canonical_a}": "probe"}}"#));
    assert!(ok, "{v2}");
    assert_eq!(v2["added"], 0);
    assert_eq!(v2["recorded"][0]["canonical"], canonical_a);
    assert_eq!(
        ledger(&root),
        stored,
        "the ledger is unchanged by the restatement"
    );

    // The next verify counts the artifact as disposed excluded.
    let report = verify(&root);
    assert_eq!(report["report"]["disposed_excluded"], 1, "{report}");
    let rationales = report["report"]["disposed_excluded_rationales"].to_string();
    assert!(rationales.contains(canonical_a), "{rationales}");
    assert!(!rationales.contains(canonical_b), "{rationales}");

    // C7: and it LEAVES the uncovered set rather than being annotated inside
    // it. A reader counts that array and a gate reads it, so an excluded
    // entry left in it stays owed however it is marked. The count rides
    // beside the figures instead, and the two renderings agree.
    let uncovered = report["report"]["coverage"]["uncovered"].to_string();
    assert!(
        !uncovered.contains(canonical_a),
        "the excluded artifact is dropped from `coverage.uncovered`: {report}"
    );
    assert!(
        uncovered.contains(canonical_b),
        "the artifact still owed stays: {report}"
    );
    assert_eq!(report["report"]["coverage"]["excluded"], 1, "{report}");
    let md = report["report_markdown"].as_str().unwrap();
    assert!(
        md.contains("- uncovered (no anchor): 1; excluded on purpose (not owed): 1"),
        "markdown agrees with the JSON count: {md}"
    );
    // Scoped to the section itself: the artifact legitimately appears
    // elsewhere in the report (the "Excluded on purpose" ledger names it with
    // its rationale, which is the point of that block).
    let uncovered_section = md
        .split("## Uncovered artifacts")
        .nth(1)
        .map(|rest| rest.split("\n## ").next().unwrap_or(rest).to_string())
        .unwrap_or_default();
    assert!(
        !uncovered_section.contains(canonical_a),
        "the excluded artifact is gone from the Uncovered artifacts section: {uncovered_section}"
    );
    assert!(
        uncovered_section.contains(canonical_b),
        "the artifact still owed is listed there: {uncovered_section}"
    );
    // This fixture renders the adopt/onboarding branch. Either wording is
    // fine; what must hold is that the headline counts ONE, not zero: the
    // renderer no longer subtracts a set the composer already removed.
    assert!(
        md.contains("1 in-scope artifact(s) carry no entity yet")
            || md.contains("1 unaccounted artifact(s)"),
        "the exhaustive line does not double-count the exclusion: {md}"
    );

    // An id resolving to no artifact refuses, naming the nearest known ids,
    // and records nothing.
    let before = ledger(&root);
    let (ok, err) = exclude(&root, r#"{"docs/zzz.md": "probe"}"#);
    assert!(!ok);
    assert_eq!(err["code"], "PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER", "{err}");
    let nearest = err["details"]["nearest"]["docs/zzz.md"].to_string();
    assert!(
        nearest.contains(canonical_a) && nearest.contains(canonical_b),
        "{err}"
    );
    assert!(
        err["message"].as_str().unwrap().contains("nearest known"),
        "{err}"
    );
    assert_eq!(ledger(&root), before, "a refused call records nothing");
}

/// A two-source binding: both sources carry `docs/a.md`, so the
/// source-relative spelling denotes two different files.
fn two_source_workspace(tmp: &TempDir) -> std::path::PathBuf {
    let root = tmp.path().join("ws");
    std::fs::create_dir_all(root.join("engine-mem").join(".memstead")).unwrap();
    std::fs::write(
        root.join("engine-mem")
            .join(".memstead")
            .join("config.json"),
        r#"{ "schema": "default@1.0.0" }"#,
    )
    .unwrap();
    std::fs::write(
        root.join("engine-mem").join("seed.md"),
        "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Seed\n\n## Identity\n\nThe seed.\n\n## Purpose\n\nExists.\n",
    )
    .unwrap();
    write_store(
        &root,
        "workspace.toml",
        "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
    );
    write_store(
        &root,
        "state/mounts.json",
        r#"{"format":"memstead-mounts-3","mounts":[{"mem":"engine","schema":"default@1.0.0","storage":{"type":"folder","path":"engine-mem"},"capability":"write","lifecycle":"eager","cross_linkable":false}]}"#,
    );
    write_store(
        &root,
        "projections/engine/docs.json",
        r#"{"version":2,"intent":"model both trees","sources":[{"name":"one","type":"codebase","pointer":"srcone","change_detection":"git","scope":[{"path":"**/*.md","mode":"allow"}]},{"name":"two","type":"codebase","pointer":"srctwo","change_detection":"git","scope":[{"path":"**/*.md","mode":"allow"}]}],"reference_mems":[],"destination_mem":"engine","deny_paths":[],"coverage_semantics":"exhaustive","operations":{"build":{"mode":"discovery","trigger":"loop","batch_size":20},"sync":{"trigger":"manual","batch_size":20},"verify":{"trigger":"manual","batch_size":20,"adjudication_mode":"strict"}}}"#,
    );
    for ptr in ["srcone", "srctwo"] {
        let src = root.join(ptr);
        std::fs::create_dir_all(src.join("docs")).unwrap();
        std::fs::write(src.join("docs").join("a.md"), format!("# A in {ptr}\n")).unwrap();
        git(&src, &["init", "-q"]);
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "init"]);
    }
    root
}

/// C8 AC1: a source-relative id carried by two sources is REFUSED naming both
/// canonical ids, and records nothing; either canonical id succeeds.
///
/// The entry was filed on the opposite behaviour: the fold took the first
/// source that matched, so the source listed earliest in the binding silently
/// won and the exclusion landed on an artifact the caller never named.
#[test]
fn an_ambiguous_source_relative_id_is_refused_naming_both_canonical_ids() {
    let tmp = TempDir::new().unwrap();
    let root = two_source_workspace(&tmp);
    let ledger_path = root
        .join(".memstead")
        .join("state")
        .join("advance")
        .join("engine")
        .join("docs.json");

    // Ambiguous: `docs/a.md` exists under both pointers.
    let (ok, err) = exclude(&root, r#"{"docs/a.md": "probe"}"#);
    assert!(!ok, "an ambiguous id must refuse: {err}");
    assert_eq!(
        err["code"], "PROJECTION_EXCLUDE_AMBIGUOUS_ARTIFACT",
        "{err}"
    );
    let named = err["details"]["ambiguous"]["docs/a.md"].to_string();
    assert!(
        named.contains("srcone/docs/a.md") && named.contains("srctwo/docs/a.md"),
        "both canonical ids are named: {err}"
    );
    // The message carries them too, for a reader who never parses details.
    let msg = err["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("srcone/docs/a.md") && msg.contains("srctwo/docs/a.md"),
        "{err}"
    );
    assert!(
        !ledger_path.exists(),
        "a refused call records nothing: the ledger must not exist"
    );

    // Either canonical id is unambiguous and succeeds.
    let (ok, v) = exclude(&root, r#"{"srctwo/docs/a.md": "probe"}"#);
    assert!(ok, "{v}");
    assert_eq!(v["recorded"][0]["canonical"], "srctwo/docs/a.md", "{v}");
    let stored: Value = serde_json::from_slice(&std::fs::read(&ledger_path).unwrap()).unwrap();
    assert!(
        stored["exclusions"]["srctwo/docs/a.md"].is_string(),
        "the id the caller named is the one recorded: {stored}"
    );
    assert!(
        stored["exclusions"]["srcone/docs/a.md"].is_null(),
        "the other source's artifact is untouched: {stored}"
    );
}

/// B11 AC1 on a sub-tree pointer inside the workspace.
#[test]
fn a_source_relative_exclude_takes_effect_and_an_unknown_id_is_refused() {
    exercise("src", "src/docs/a.md", "src/docs/b.md");
}

/// B11 AC1 on a pointer outside the workspace root, the flagship's shape.
#[test]
fn the_same_holds_for_a_pointer_outside_the_workspace_root() {
    exercise("../ext", "../ext/docs/a.md", "../ext/docs/b.md");
}

/// B11 AC1 refusal complement: a ledger written before the plan, holding
/// the canonical form, still filters the same artifact.
#[test]
fn a_pre_existing_canonical_ledger_still_filters() {
    let tmp = TempDir::new().unwrap();
    let root = bound_workspace(&tmp, "../ext");
    write_store(
        &root,
        "state/advance/engine/docs.json",
        r#"{"binding":"engine/docs","frozen_slice":{"added":[],"modified":[],"deleted":[]},"dispositions":{},"exclusions":{"../ext/docs/b.md":"written before the plan"},"exclusion_sources":{"../ext/docs/b.md":"docs"}}"#,
    );
    let report = verify(&root);
    assert_eq!(report["report"]["disposed_excluded"], 1, "{report}");
    assert!(
        report["report"]["disposed_excluded_rationales"]
            .to_string()
            .contains("../ext/docs/b.md"),
        "{report}"
    );
}
