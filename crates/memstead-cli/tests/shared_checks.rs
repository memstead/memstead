//! A git-branch mem keeps its check rows in the store that holds it, on
//! one ref per mem beside its branch: every workspace that mounts the mem
//! reads the same rows, concurrent recorders lose none, a copy of the
//! mem-repo and an adopted mem carry them, and the rows a workspace's
//! ledger file held before move into the store once.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead() -> Command {
    Command::cargo_bin("memstead").expect("memstead binary must be built by cargo")
}

fn run(ws: &Path, args: &[&str]) -> (bool, String) {
    let out = memstead()
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ),
    )
}

fn ok(ws: &Path, args: &[&str]) -> String {
    let (succeeded, printed) = run(ws, args);
    assert!(
        succeeded,
        "`memstead {}` failed:\n{printed}",
        args.join(" ")
    );
    printed
}

fn json(ws: &Path, args: &[&str]) -> serde_json::Value {
    let out = memstead()
        .current_dir(ws)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`memstead --json {}` failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn git(gitdir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(gitdir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sha(gitdir: &Path, ref_name: &str) -> String {
    git(gitdir, &["rev-parse", "--verify", "--quiet", ref_name])
}

fn spec(ws: &Path, mem: &str, title: &str, identity: &str) {
    ok(
        ws,
        &[
            "create",
            "--mem",
            mem,
            "--title",
            title,
            "--type",
            "spec",
            "--metadata",
            "level=M0",
            "--section",
            "identity=i",
            "--section",
            "purpose=p",
            "--identity",
            identity,
        ],
    );
}

/// A store whose mem-repo holds the mem `kept` with the entities named.
fn store(root: &Path, name: &str, titles: &[&str]) -> (PathBuf, PathBuf) {
    let store = root.join(name);
    fs::create_dir_all(&store).unwrap();
    ok(&store, &["mem-repo", "init", "."]);
    ok(&store, &["workspace", "allow-create", "--schema", "*", "*"]);
    ok(
        &store,
        &["mem", "init", "kept", "--schema", "default@1.3.0"],
    );
    for title in titles {
        spec(&store, "kept", title, "author-a");
    }
    let gitdir = store.join("mem-repo/.git").canonicalize().unwrap();
    (store, gitdir)
}

/// A view: a workspace of engine state only mounting `kept` of `gitdir`.
fn view(root: &Path, name: &str, gitdir: &Path, capability: &str) -> PathBuf {
    let view = root.join(name);
    fs::create_dir_all(&view).unwrap();
    ok(
        &view,
        &[
            "mem",
            "mount",
            "kept",
            "--branch",
            "kept",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            capability,
            "--new-workspace",
            "--operator-mode",
        ],
    );
    view
}

fn rows(ws: &Path) -> Vec<serde_json::Value> {
    json(ws, &["checks", "--mem", "kept"])["checks"]
        .as_array()
        .unwrap()
        .clone()
}

/// A check recorded through one view reads the same, by every reader of
/// the list, in every workspace of the list: another writable view, a
/// read-only view, the store's own workspace, and a workspace of another
/// store that mounts the mem read-only. An engine already running there
/// reads it at its next operation. The check moves no branch, no
/// registry ref and no entity hash.
#[test]
fn a_check_recorded_in_one_workspace_reads_the_same_in_every_workspace() {
    let tmp = TempDir::new().unwrap();
    let (store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    let c = view(tmp.path(), "c", &gitdir, "write");
    let d = view(tmp.path(), "d", &gitdir, "write");
    let r = view(tmp.path(), "r", &gitdir, "read-only");
    // Another store's workspace: its own mem-repo, `kept` mounted
    // read-only from the first store's git directory.
    let other = tmp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    ok(&other, &["mem-repo", "init", "."]);
    ok(
        &other,
        &[
            "mem",
            "mount",
            "kept",
            "--branch",
            "kept",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            "read-only",
            "--operator-mode",
        ],
    );
    let running = memstead_git_branch::workspace_store::engine_from_workspace_root(&d).unwrap();
    assert!(running.check_records("kept").0.is_empty());

    let branch_before = sha(&gitdir, "refs/heads/kept");
    let registry_before = sha(&gitdir, "refs/heads/__MEMSTEAD");
    let hash_before = json(&c, &["entity", "kept--thing"])["_hash"].clone();
    ok(
        &c,
        &[
            "check",
            "kept--thing",
            "--verdict",
            "ok",
            "--identity",
            "checker-c",
            "--role",
            "checker",
        ],
    );
    assert_eq!(sha(&gitdir, "refs/heads/kept"), branch_before);
    assert_eq!(sha(&gitdir, "refs/heads/__MEMSTEAD"), registry_before);
    assert_eq!(json(&c, &["entity", "kept--thing"])["_hash"], hash_before);
    assert!(!sha(&gitdir, "refs/memstead/checks/kept").is_empty());

    let recorded = rows(&c);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    for ws in [&d, &r, &store, &other] {
        assert_eq!(rows(ws), recorded, "{}", ws.display());
        let prov = &json(ws, &["entity", "kept--thing", "--provenance"])["mutation_provenance"];
        assert_eq!(
            prov["check_state"],
            "checked_ok",
            "{}: {prov}",
            ws.display()
        );
        assert_eq!(prov["last_check"]["identity"], "checker-c", "{prov}");
        assert_eq!(prov["last_check"]["role"], "checker", "{prov}");
        assert_eq!(
            prov["last_check"]["checked_write"], recorded[0]["checked_write"],
            "{prov}"
        );
    }
    // The running engine reads the row at its next operation.
    let seen = running.check_records("kept").0;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].identity.as_deref(), Some("checker-c"));
    assert!(
        running
            .latest_check_record("kept", "kept--thing", "verification")
            .is_some()
    );
}

/// Four processes in two views record 16 checks each on one mem, started
/// together: all 64 rows land, each once, and the mem-repo stays sound.
/// Five rounds.
#[test]
fn concurrent_checks_from_several_processes_are_all_kept() {
    for round in 0..5 {
        let tmp = TempDir::new().unwrap();
        let (_store, gitdir) = store(tmp.path(), "store", &["One", "Two", "Three", "Four"]);
        let views = [
            view(tmp.path(), "v1", &gitdir, "write"),
            view(tmp.path(), "v2", &gitdir, "write"),
        ];
        let mut children = Vec::new();
        for p in 0..4 {
            let ws = &views[p % 2];
            let entries: Vec<serde_json::Value> = (0..16)
                .map(|i| {
                    let entity = ["kept--one", "kept--two", "kept--three", "kept--four"][i % 4];
                    serde_json::json!({
                        "id": entity,
                        "verdict": "ok",
                        "kind": format!("x-k{i}")
                    })
                })
                .collect();
            let file = ws.join(format!("batch-{p}.json"));
            fs::write(&file, serde_json::json!({ "checks": entries }).to_string()).unwrap();
            let bin = assert_cmd::cargo::cargo_bin("memstead");
            children.push(
                std::process::Command::new(bin)
                    .current_dir(ws)
                    .args([
                        "check",
                        "--from",
                        file.to_str().unwrap(),
                        "--identity",
                        &format!("p{p}"),
                        "--quiet",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
        }
        for child in children {
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "round {round}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        for ws in &views {
            let seen = rows(ws);
            assert_eq!(seen.len(), 64, "round {round}");
            let distinct: std::collections::BTreeSet<String> =
                seen.iter().map(|r| r.to_string()).collect();
            assert_eq!(distinct.len(), 64, "round {round}: a row twice");
        }
        let fsck = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&gitdir)
            .args(["fsck", "--no-progress"])
            .output()
            .unwrap();
        assert!(
            fsck.status.success(),
            "round {round}: {}",
            String::from_utf8_lossy(&fsck.stderr)
        );
    }
}

/// A mirror of the mem-repo carries the checks; a mem adopted by
/// `mem mount --remote` brings them, with the same standing.
#[test]
fn the_mems_checks_travel_with_a_copy_and_with_an_adopted_mem() {
    let tmp = TempDir::new().unwrap();
    let (store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    ok(
        &store,
        &[
            "check",
            "kept--thing",
            "--verdict",
            "ok",
            "--identity",
            "checker-c",
            "--role",
            "checker",
        ],
    );
    let recorded = rows(&store);
    assert_eq!(recorded.len(), 1);

    // (a) A mirror, mounted by a fresh view.
    let mirror = tmp.path().join("mirror.git");
    let status = std::process::Command::new("git")
        .args(["clone", "--mirror", "--quiet"])
        .arg(&gitdir)
        .arg(&mirror)
        .status()
        .unwrap();
    assert!(status.success());
    let mirror = mirror.canonicalize().unwrap();
    let on_mirror = view(tmp.path(), "on-mirror", &mirror, "read-only");
    assert_eq!(rows(&on_mirror), recorded);

    // (b) Adopted into another store from a remote.
    let target = tmp.path().join("target");
    fs::create_dir_all(&target).unwrap();
    ok(&target, &["mem-repo", "init", "."]);
    ok(
        &target,
        &["workspace", "allow-create", "--schema", "*", "*"],
    );
    ok(
        &target,
        &["mem-repo", "remote-add", "src", gitdir.to_str().unwrap()],
    );
    ok(
        &target,
        &[
            "mem",
            "mount",
            "kept",
            "--remote",
            "src",
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    assert_eq!(rows(&target), recorded);
    let prov = &json(&target, &["entity", "kept--thing", "--provenance"])["mutation_provenance"];
    assert_eq!(prov["check_state"], "checked_ok", "{prov}");
    let target_gitdir = target.join("mem-repo/.git");
    assert!(!sha(&target_gitdir, "refs/memstead/checks/kept").is_empty());
}

/// The rows a workspace's ledger file holds for a writable git-branch mem
/// move into the mem's store when an engine first opens the workspace;
/// the explicit migration adds nothing after that, and rows of a mem the
/// workspace does not mount stay in the file and nowhere else.
#[test]
fn rows_held_in_a_workspace_ledger_reach_the_store_once() {
    let tmp = TempDir::new().unwrap();
    let (_store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    let v = view(tmp.path(), "v", &gitdir, "write");
    let hash = json(&v, &["entity", "kept--thing"])["_hash"]
        .as_str()
        .unwrap()
        .to_string();
    // A ledger file as 0.28.0 wrote it: a row of `kept` and a row of a
    // mem this workspace does not mount.
    let checks = v.join(".memstead/state/checks");
    fs::create_dir_all(&checks).unwrap();
    let line = |entity: &str, h: &str| {
        format!(
            "{{\"ts\":1790000000,\"entity\":\"{entity}\",\"verdict\":\"ok\",\"entity_hash\":\"{h}\",\
             \"actor\":\"cli\",\"role\":\"checker\",\"identity\":\"checker-old\"}}\n"
        )
    };
    fs::write(
        checks.join("checks.jsonl"),
        format!(
            "{}{}",
            line("kept--thing", &hash),
            line("ghost--gone", "h0")
        ),
    )
    .unwrap();
    let _ = fs::remove_file(checks.join("store-migrated"));
    assert!(sha(&gitdir, "refs/memstead/checks/kept").is_empty());

    // The first opening moves the row of `kept`.
    let seen = rows(&v);
    assert_eq!(seen.len(), 1, "{seen:?}");
    let in_store = git(&gitdir, &["show", "refs/memstead/checks/kept:checks.jsonl"]);
    assert_eq!(in_store.lines().count(), 1, "{in_store}");
    assert!(in_store.contains("checker-old"));
    let prov = &json(&v, &["entity", "kept--thing", "--provenance"])["mutation_provenance"];
    assert_eq!(prov["check_state"], "checked_ok", "{prov}");

    // The explicit migration and a second opening add nothing.
    let moved = json(&v, &["checks", "--migrate"]);
    assert!(
        moved["migrated"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["moved"] == 0),
        "{moved}"
    );
    let in_store = git(&gitdir, &["show", "refs/memstead/checks/kept:checks.jsonl"]);
    assert_eq!(in_store.lines().count(), 1, "{in_store}");
    assert_eq!(rows(&v).len(), 1);
    // The unmounted mem's row stays in the file and nowhere else.
    assert!(sha(&gitdir, "refs/memstead/checks/ghost").is_empty());
    assert!(
        fs::read_to_string(checks.join("checks.jsonl"))
            .unwrap()
            .contains("ghost--gone")
    );
    let ghost = json(&v, &["checks", "--mem", "ghost"]);
    assert_eq!(ghost["checks"].as_array().unwrap().len(), 1, "{ghost}");

    // Another view of the same store reads the moved row at once.
    let w = view(tmp.path(), "w", &gitdir, "read-only");
    assert_eq!(rows(&w).len(), 1);
}

/// A row of the remote's checks ref dated beyond the clock skew is not
/// imported by `mem mount --remote`, and the mount names it; a row
/// within the skew lands.
#[test]
fn a_mount_from_a_remote_names_a_future_row_it_refused() {
    let tmp = TempDir::new().unwrap();
    let (store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    ok(
        &store,
        &[
            "check",
            "kept--thing",
            "--verdict",
            "ok",
            "--identity",
            "checker-c",
            "--role",
            "checker",
        ],
    );
    let held = git(&gitdir, &["show", "refs/memstead/checks/kept:checks.jsonl"]);
    let ahead = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 86_400;
    let doctored = format!(
        "{held}\n{{\"ts\":{ahead},\"entity\":\"kept--thing\",\"verdict\":\"failed\",\
         \"entity_hash\":\"0000000000000000\",\"actor\":\"cli\",\"role\":\"checker\",\
         \"identity\":\"time-traveller\"}}\n"
    );
    let file = tmp.path().join("doctored.jsonl");
    fs::write(&file, doctored).unwrap();
    let blob = git(&gitdir, &["hash-object", "-w", file.to_str().unwrap()]);
    let mut mktree = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&gitdir)
        .arg("mktree")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        mktree
            .stdin
            .take()
            .unwrap()
            .write_all(format!("100644 blob {blob}\tchecks.jsonl\n").as_bytes())
            .unwrap();
    }
    let tree = String::from_utf8(mktree.wait_with_output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_string();
    let commit = git(
        &gitdir,
        &[
            "-c",
            "user.name=x",
            "-c",
            "user.email=x@x",
            "commit-tree",
            &tree,
            "-p",
            "refs/memstead/checks/kept",
            "-m",
            "a wrong clock",
        ],
    );
    git(
        &gitdir,
        &["update-ref", "refs/memstead/checks/kept", &commit],
    );

    let target = tmp.path().join("target");
    fs::create_dir_all(&target).unwrap();
    ok(&target, &["mem-repo", "init", "."]);
    ok(
        &target,
        &["workspace", "allow-create", "--schema", "*", "*"],
    );
    ok(
        &target,
        &["mem-repo", "remote-add", "src", gitdir.to_str().unwrap()],
    );
    let mounted = json(
        &target,
        &[
            "mem",
            "mount",
            "kept",
            "--remote",
            "src",
            "--capability",
            "write",
            "--operator-mode",
        ],
    );
    assert_eq!(mounted["checks_imported"], 1, "{mounted}");
    let refused = mounted["checks_refused"].as_array().unwrap();
    assert_eq!(refused.len(), 1, "{mounted}");
    assert_eq!(refused[0]["identity"], "time-traveller", "{mounted}");
    let seen = rows(&target);
    assert_eq!(seen.len(), 1);
    assert!(!seen.iter().any(|r| r["identity"] == "time-traveller"));
}

/// A view mounting `kept` with a capability and a check grant.
fn granted_view(root: &Path, name: &str, gitdir: &Path, capability: &str, check: &str) -> PathBuf {
    let view = root.join(name);
    fs::create_dir_all(&view).unwrap();
    ok(
        &view,
        &[
            "mem",
            "mount",
            "kept",
            "--branch",
            "kept",
            "--gitdir",
            gitdir.to_str().unwrap(),
            "--capability",
            capability,
            "--check",
            check,
            "--new-workspace",
            "--operator-mode",
        ],
    );
    view
}

fn refused_with(ws: &Path, args: &[&str], code: &str) {
    let (succeeded, printed) = run(ws, args);
    assert!(
        !succeeded,
        "`memstead {}` answered:\n{printed}",
        args.join(" ")
    );
    assert!(
        printed.contains(code),
        "`memstead {}` should refuse {code}:\n{printed}",
        args.join(" ")
    );
}

/// Refs of a git directory but the checks refs.
fn refs_but_checks(gitdir: &Path) -> String {
    git(
        gitdir,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    )
    .lines()
    .filter(|l| !l.starts_with("refs/memstead/checks/"))
    .collect::<Vec<_>>()
    .join("\n")
}

/// A read-only mount granted `check` records checks of every kind, which
/// every other workspace reads; every mutation of the list still refuses
/// `READ_ONLY_MOUNT`; nothing in the store moves but the checks ref.
#[test]
fn a_read_only_mount_granted_checking_records_checks_and_nothing_else() {
    let tmp = TempDir::new().unwrap();
    let (store, gitdir) = store(tmp.path(), "store", &["Thing", "Other"]);
    let r = granted_view(tmp.path(), "r", &gitdir, "read-only", "true");
    let listed = json(&r, &["mem", "list"]);
    assert_eq!(listed["mems"][0]["check"], true, "{listed}");
    let before = refs_but_checks(&gitdir);
    for kind in ["verification", "conformance", "x-approval"] {
        ok(
            &r,
            &[
                "check",
                "kept--thing",
                "--verdict",
                "ok",
                "--kind",
                kind,
                "--identity",
                "rev",
                "--role",
                "checker",
            ],
        );
    }
    assert_eq!(rows(&r).len(), 3);
    assert_eq!(rows(&store).len(), 3);
    let spec_args = [
        "create",
        "--mem",
        "kept",
        "--title",
        "New",
        "--type",
        "spec",
        "--metadata",
        "level=M0",
        "--section",
        "identity=i",
        "--section",
        "purpose=p",
    ];
    refused_with(&r, &spec_args, "READ_ONLY_MOUNT");
    for args in [
        vec![
            "update",
            "kept--thing",
            "--section",
            "purpose=q",
            "--auto-hash",
        ],
        vec!["relate", "kept--thing", "USES", "kept--other"],
        vec!["delete", "kept--other"],
        vec!["rename", "kept--thing", "Renamed"],
        vec!["retype", "kept--thing", "--type", "concept"],
        vec!["mem", "set-version", "kept", "0.2.0"],
        vec!["mem", "set-schema", "kept", "default@1.3.0"],
    ] {
        refused_with(&r, &args, "READ_ONLY_MOUNT");
    }
    let batch = r.join("b.json");
    fs::write(
        &batch,
        r#"{"updates":[{"id":"kept--thing","auto_hash":true,"sections":{"purpose":"q"}}]}"#,
    )
    .unwrap();
    refused_with(
        &r,
        &["batch-update", "--from", batch.to_str().unwrap()],
        "READ_ONLY_MOUNT",
    );
    assert_eq!(refs_but_checks(&gitdir), before, "a ref moved");
    let entity = json(&r, &["entity", "kept--thing"]);
    assert_eq!(entity["origin"], "third-party", "{entity}");
}

/// A writable mount denied `check` writes, and every check refuses
/// `CHECK_NOT_GRANTED` with nothing recorded anywhere.
#[test]
fn a_writable_mount_denied_checking_writes_and_refuses_every_check() {
    let tmp = TempDir::new().unwrap();
    let (store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    let w = granted_view(tmp.path(), "w", &gitdir, "write", "false");
    let listed = json(&w, &["mem", "list"]);
    assert_eq!(listed["mems"][0]["check"], false, "{listed}");
    ok(
        &w,
        &[
            "update",
            "kept--thing",
            "--section",
            "purpose=q",
            "--auto-hash",
        ],
    );
    spec(&w, "kept", "Written", "writer");
    for kind in ["verification", "conformance", "x-approval"] {
        refused_with(
            &w,
            &["check", "kept--thing", "--verdict", "ok", "--kind", kind],
            "CHECK_NOT_GRANTED",
        );
    }
    assert!(rows(&w).is_empty());
    assert!(rows(&store).is_empty());
    assert!(sha(&gitdir, "refs/memstead/checks/kept").is_empty());
    assert!(!w.join(".memstead/state/checks/checks.jsonl").exists());
}

/// A grant another process changes on the roster takes effect at the
/// running engine's next operation.
#[test]
fn a_grant_changed_by_another_process_takes_effect_at_the_next_operation() {
    use memstead_base::check::{CheckKind, Verdict};
    use memstead_base::vcs::Actor;
    let tmp = TempDir::new().unwrap();
    let (_store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    let v = view(tmp.path(), "v", &gitdir, "write");
    let mut engine = memstead_git_branch::workspace_store::engine_from_workspace_root(&v).unwrap();
    let record = |engine: &mut memstead_base::Engine| {
        engine
            .record_check(
                "kept",
                "kept--thing",
                Verdict::Ok,
                CheckKind::Verification,
                None,
                Actor::Cli,
                None,
            )
            .map(|_| ())
            .map_err(|e| e.code().to_string())
    };
    assert_eq!(record(&mut engine), Ok(()));
    let roster = v.join(".memstead/state/mounts.json");
    let set = |f: &dyn Fn(&mut serde_json::Value)| {
        let mut doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&roster).unwrap()).unwrap();
        f(&mut doc["mounts"][0]);
        fs::write(&roster, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    };
    set(&|row| row["check"] = serde_json::json!(false));
    assert_eq!(record(&mut engine), Err("CHECK_NOT_GRANTED".to_string()));
    set(&|row| row["check"] = serde_json::json!(true));
    assert_eq!(record(&mut engine), Ok(()));
    set(&|row| {
        row["capability"] = serde_json::json!("read-only");
        row.as_object_mut().unwrap().remove("check");
    });
    assert_eq!(record(&mut engine), Err("READ_ONLY_MOUNT".to_string()));
    set(&|row| row["check"] = serde_json::json!(true));
    assert_eq!(record(&mut engine), Ok(()));
    assert!(engine.mount("kept").is_some());
}

/// One `tools/call` against a running `memstead-mcp` over stdio, by a
/// minimal line-delimited JSON-RPC driver.
struct Mcp {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    reader: std::io::BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(ws: &Path) -> Self {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("memstead-mcp"))
            .arg("--workspace")
            .arg(ws)
            .current_dir(ws)
            .env_remove("MEMSTEAD_WORKSPACE")
            .env_remove("MEMSTEAD_IDENTITY")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("memstead-mcp spawns");
        let stdin = child.stdin.take().unwrap();
        let reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut s = Self {
            child,
            stdin,
            reader,
            next_id: 0,
        };
        s.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": { "name": "shared-checks-test", "version": "0" }
            }),
        );
        s.send(&serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        s
    }

    fn send(&mut self, m: &serde_json::Value) {
        use std::io::Write as _;
        writeln!(self.stdin, "{m}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        use std::io::BufRead as _;
        self.next_id += 1;
        let id = self.next_id;
        self.send(
            &serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        );
        loop {
            let mut line = String::new();
            assert!(
                self.reader.read_line(&mut line).unwrap() > 0,
                "memstead-mcp exited"
            );
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
                && v["id"] == serde_json::json!(id)
            {
                return v["result"].clone();
            }
        }
    }

    fn call(&mut self, tool: &str, arguments: serde_json::Value) -> serde_json::Value {
        self.request(
            "tools/call",
            serde_json::json!({ "name": tool, "arguments": arguments }),
        )
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A running server whose next call after a grant change is a check:
/// the check honours the new row, and its reply names the roster change,
/// on a refusal and on a recorded check alike.
#[test]
fn a_check_after_a_grant_change_names_the_roster_change() {
    let tmp = TempDir::new().unwrap();
    let (_store, gitdir) = store(tmp.path(), "store", &["Thing"]);
    let v = view(tmp.path(), "v", &gitdir, "write");
    let mut mcp = Mcp::start(&v);
    let check = serde_json::json!({ "entity": "kept--thing", "verdict": "ok" });
    let first = mcp.call("memstead_check", check.clone());
    assert_ne!(first["isError"], true, "{first}");
    let roster = v.join(".memstead/state/mounts.json");
    let set = |value: bool| {
        let mut doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&roster).unwrap()).unwrap();
        doc["mounts"][0]["check"] = serde_json::json!(value);
        fs::write(&roster, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    };
    set(false);
    let refused = mcp.call("memstead_check", check.clone());
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(
        refused["structuredContent"]["code"], "CHECK_NOT_GRANTED",
        "{refused}"
    );
    assert!(
        refused.to_string().contains("MEM_ROSTER_CHANGED"),
        "{refused}"
    );
    assert!(refused.to_string().contains("kept"), "{refused}");
    set(true);
    let recorded = mcp.call("memstead_check", check);
    assert_ne!(recorded["isError"], true, "{recorded}");
    assert!(
        recorded.to_string().contains("MEM_ROSTER_CHANGED"),
        "{recorded}"
    );
}
