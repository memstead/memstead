//! Integration tests for portable installed-mem mounts: a read-only mem
//! installed into the archive cache is recorded in the tracked mount
//! roster by its content-addressed file name, never by the absolute
//! path of one machine's cache, so the same roster loads on any
//! machine. Two cache directories (`MEMSTEAD_MEM_CACHE`) stand in for
//! two machines.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

fn memstead(cache: &Path) -> Command {
    let mut cmd = Command::cargo_bin("memstead").expect("memstead binary must be built by cargo");
    cmd.env("MEMSTEAD_MEM_CACHE", cache);
    cmd
}

fn run(ws: &Path, cache: &Path, args: &[&str]) -> std::process::Output {
    memstead(cache)
        .current_dir(ws)
        .args(args)
        .arg("--quiet")
        .output()
        .unwrap()
}

fn ok(ws: &Path, cache: &Path, args: &[&str]) -> String {
    let out = run(ws, cache, args);
    assert!(
        out.status.success(),
        "`memstead {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn path(&self, p: &str) -> PathBuf {
        self.root.path().join(p)
    }
}

/// A source workspace with mem `recipes` exported to `recipes.mem`, and
/// a variant export with different content (one more entity) to
/// `recipes-other.mem`. Then workspace `w1` installs `recipes.mem`
/// into cache `c1`.
fn fixture() -> Fixture {
    let f = Fixture {
        root: TempDir::new().unwrap(),
    };
    let src = f.path("src");
    let c0 = f.path("c0");
    fs::create_dir_all(&src).unwrap();
    ok(&src, &c0, &["mem-repo", "init", "."]);
    ok(
        &src,
        &c0,
        &["workspace", "allow-create", "--schema", "*", "r*"],
    );
    ok(
        &src,
        &c0,
        &["mem", "init", "recipes", "--schema", "default@1.3.0"],
    );
    ok(
        &src,
        &c0,
        &[
            "create",
            "--mem",
            "recipes",
            "--title",
            "Bread",
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
    let archive = f.path("recipes.mem");
    ok(
        &src,
        &c0,
        &[
            "export",
            "--format",
            "mem",
            "--mem",
            "recipes",
            "-o",
            archive.to_str().unwrap(),
        ],
    );
    ok(
        &src,
        &c0,
        &[
            "create",
            "--mem",
            "recipes",
            "--title",
            "Soup",
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
    let other = f.path("recipes-other.mem");
    ok(
        &src,
        &c0,
        &[
            "export",
            "--format",
            "mem",
            "--mem",
            "recipes",
            "-o",
            other.to_str().unwrap(),
        ],
    );

    let w1 = f.path("w1");
    fs::create_dir_all(&w1).unwrap();
    ok(&w1, &f.path("c1"), &["mem-repo", "init", "."]);
    ok(&w1, &f.path("c1"), &["install", archive.to_str().unwrap()]);
    f
}

fn roster(ws: &Path) -> String {
    fs::read_to_string(ws.join(".memstead/state/mounts.json")).unwrap()
}

fn quarantine_of(ws: &Path, cache: &Path, mem: &str) -> Option<(String, String)> {
    let out = run(ws, cache, &["status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["quarantined"].as_array().and_then(|a| {
        a.iter().find(|q| q["mem"] == mem).map(|q| {
            (
                q["reason_code"].as_str().unwrap_or_default().to_string(),
                q["reason"].as_str().unwrap_or_default().to_string(),
            )
        })
    })
}

/// 02-a, assertion: the roster written on machine one mounts the same
/// archive on machine two once it is installed there, and booting there
/// leaves the roster bytes unchanged.
#[test]
fn the_same_roster_mounts_the_same_archive_on_a_second_machine() {
    let f = fixture();
    let (w1, c1, c2) = (f.path("w1"), f.path("c1"), f.path("c2"));
    let written = roster(&w1);
    assert!(written.contains("\"cached-archive\""), "{written}");
    assert!(!written.contains(c1.to_str().unwrap()), "{written}");

    let w2 = f.path("w2");
    copy_dir(&w1, &w2);
    ok(
        &w2,
        &c2,
        &["install", f.path("recipes.mem").to_str().unwrap()],
    );
    ok(&w2, &c2, &["status"]);
    let a = ok(&w1, &c1, &["entity", "recipes--bread", "--json"]);
    let b = ok(&w2, &c2, &["entity", "recipes--bread", "--json"]);
    let hash = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap()["_hash"].clone();
    assert_eq!(hash(&a), hash(&b));
    assert_eq!(
        roster(&w2),
        written,
        "machine two's boot and install keep the roster"
    );
}

/// 02-a, refusal complement: an archive of the same mem name but other
/// content in machine two's cache is not mounted in its place.
#[test]
fn an_archive_with_other_content_is_never_mounted_in_its_place() {
    let f = fixture();
    let (w1, c3) = (f.path("w1"), f.path("c3"));
    let w3 = f.path("w3");
    copy_dir(&w1, &w3);
    ok(
        &w3,
        &c3,
        &["install", f.path("recipes-other.mem").to_str().unwrap()],
    );
    // Installing the other content re-points this workspace's mount; a
    // fresh copy of machine one's roster still names the first archive.
    let w4 = f.path("w4");
    copy_dir(&w1, &w4);
    let (code, message) = quarantine_of(&w4, &c3, "recipes").expect("quarantined");
    assert_eq!(code, "ARCHIVE_NOT_INSTALLED");
    assert!(
        message.contains("does not match the recorded identity"),
        "{message}"
    );
    let out = run(&w4, &c3, &["entity", "recipes--soup"]);
    assert!(!out.status.success(), "the other archive must not serve");
}

/// 02-b: a missing installed mem says what to install, installing it
/// brings it into service with the roster untouched, and boot never
/// reaches for the network.
#[test]
fn a_missing_installed_mem_says_what_to_install() {
    let f = fixture();
    let (w1, c2) = (f.path("w1"), f.path("c2"));
    let w2 = f.path("w2");
    copy_dir(&w1, &w2);
    let before = roster(&w2);

    // An unroutable registry: a boot that tried the network would stall
    // or fail; it does neither.
    let out = memstead(&c2)
        .current_dir(&w2)
        .env("MEMSTEAD_REGISTRY", "http://127.0.0.1:1")
        .args(["status", "--json", "--quiet"])
        .timeout(std::time::Duration::from_secs(20))
        .output()
        .unwrap();
    assert!(out.status.success());
    let (code, message) = quarantine_of(&w2, &c2, "recipes").expect("quarantined");
    assert_eq!(code, "ARCHIVE_NOT_INSTALLED");
    assert!(message.contains("memstead install"), "{message}");
    assert!(message.contains("recipes-"), "{message}");

    ok(
        &w2,
        &c2,
        &["install", f.path("recipes.mem").to_str().unwrap()],
    );
    assert!(quarantine_of(&w2, &c2, "recipes").is_none());
    ok(&w2, &c2, &["entity", "recipes--bread"]);
    assert_eq!(roster(&w2), before);
}

/// 02-b, refusal complement: storage that is not a cached archive keeps
/// reporting MOUNT_UNBACKED when it is gone.
#[test]
fn other_missing_storage_stays_mount_unbacked() {
    let f = fixture();
    let (w1, c1) = (f.path("w1"), f.path("c1"));
    let path = w1.join(".memstead/state/mounts.json");
    let mut v: serde_json::Value = serde_json::from_str(&roster(&w1)).unwrap();
    let mounts = v["mounts"].as_array_mut().unwrap();
    mounts.push(serde_json::json!({
        "mem": "elsewhere-archive",
        "schema": "default@1.3.0",
        "storage": {"type": "archive", "path": f.path("gone/elsewhere.mem")},
        "capability": "read-only", "lifecycle": "eager", "cross_linkable": false
    }));
    mounts.push(serde_json::json!({
        "mem": "elsewhere-folder",
        "schema": "default@1.3.0",
        "storage": {"type": "folder", "path": f.path("gone/folder")},
        "capability": "read-only", "lifecycle": "eager", "cross_linkable": false
    }));
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    for mem in ["elsewhere-archive", "elsewhere-folder"] {
        let (code, _) = quarantine_of(&w1, &c1, mem).expect("quarantined");
        assert_eq!(code, "MOUNT_UNBACKED", "{mem}");
    }
}

/// 02-c: a roster in the previous format with an absolute cache path
/// loads unchanged, and the next roster write stores the identity form;
/// an archive at an explicit location keeps its path; no roster the
/// engine writes holds an absolute path into the cache.
#[test]
fn old_rosters_load_and_new_rosters_carry_no_cache_path() {
    let f = fixture();
    let (w1, c1) = (f.path("w1"), f.path("c1"));
    let cached = fs::read_dir(&c1)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "mem"))
        .unwrap();
    let explicit = f.path("explicit/recipes-copy.mem");
    fs::create_dir_all(explicit.parent().unwrap()).unwrap();
    fs::copy(f.path("recipes-other.mem"), &explicit).unwrap();
    let old = serde_json::json!({
        "format": "memstead-mounts-3",
        "mounts": [
            {"mem": "recipes", "schema": "default@1.3.0",
             "storage": {"type": "archive", "path": cached},
             "capability": "read-only", "lifecycle": "eager", "cross_linkable": false},
            {"mem": "pinned", "schema": "default@1.3.0",
             "storage": {"type": "archive", "path": explicit},
             "capability": "read-only", "lifecycle": "eager", "cross_linkable": false}
        ]
    });
    fs::write(
        w1.join(".memstead/state/mounts.json"),
        serde_json::to_string_pretty(&old).unwrap(),
    )
    .unwrap();
    ok(&w1, &c1, &["entity", "recipes--bread"]);
    assert!(quarantine_of(&w1, &c1, "recipes").is_none());

    // A roster write: a writable mem joins the workspace.
    ok(
        &w1,
        &c1,
        &["workspace", "allow-create", "--schema", "*", "n*"],
    );
    ok(
        &w1,
        &c1,
        &["mem", "init", "notes", "--schema", "default@1.3.0"],
    );
    let written = roster(&w1);
    assert!(written.contains("memstead-mounts-4"), "{written}");
    assert!(written.contains("\"cached-archive\""), "{written}");
    assert!(!written.contains(c1.to_str().unwrap()), "{written}");
    assert!(
        written.contains(explicit.to_str().unwrap()),
        "an explicit location keeps its path: {written}"
    );
    ok(&w1, &c1, &["entity", "recipes--bread"]);
}

/// An installed mem this machine does not hold can be uninstalled
/// here: the roster entry goes, and no later state write brings it
/// back.
#[test]
fn a_missing_installed_mem_can_be_uninstalled() {
    let f = fixture();
    let (w1, c2) = (f.path("w1"), f.path("c2"));
    let w2 = f.path("w2");
    copy_dir(&w1, &w2);
    assert!(quarantine_of(&w2, &c2, "recipes").is_some());
    ok(&w2, &c2, &["uninstall", "recipes"]);
    assert!(!roster(&w2).contains("\"recipes\""), "{}", roster(&w2));
    ok(
        &w2,
        &c2,
        &["workspace", "allow-create", "--schema", "*", "n*"],
    );
    ok(
        &w2,
        &c2,
        &["mem", "init", "notes", "--schema", "default@1.3.0"],
    );
    assert!(!roster(&w2).contains("\"recipes\""), "{}", roster(&w2));
}

/// A mem pinned to a sealed schema no built-in provides (`kitchen`),
/// exported twice with different content, then installed from cache
/// `c1` into a FOLDER workspace `fw1`, the shape with no schema ref to
/// fall back on.
fn sealed_fixture() -> Fixture {
    let f = Fixture {
        root: TempDir::new().unwrap(),
    };
    let (src, c0) = (f.path("src"), f.path("c0"));
    fs::create_dir_all(&src).unwrap();
    ok(&src, &c0, &["mem-repo", "init", "."]);
    ok(
        &src,
        &c0,
        &["workspace", "allow-create", "--schema", "*", "p*"],
    );
    ok(&src, &c0, &["schema", "new", "kitchen"]);
    ok(&src, &c0, &["schema", "install", "kitchen"]);
    ok(
        &src,
        &c0,
        &["mem", "init", "pantry", "--schema", "kitchen@0.1.0"],
    );
    ok(
        &src,
        &c0,
        &[
            "create",
            "--mem",
            "pantry",
            "--title",
            "Flour",
            "--type",
            "note",
            "--section",
            "summary=Flour.",
        ],
    );
    let export = |out: &str| {
        let p = f.path(out);
        ok(
            &src,
            &c0,
            &[
                "export",
                "--format",
                "mem",
                "--mem",
                "pantry",
                "-o",
                p.to_str().unwrap(),
            ],
        );
    };
    export("pantry.mem");
    ok(
        &src,
        &c0,
        &[
            "create",
            "--mem",
            "pantry",
            "--title",
            "Salt",
            "--type",
            "note",
            "--section",
            "summary=Salt.",
        ],
    );
    export("pantry-other.mem");

    let fw1 = f.path("fw1");
    fs::create_dir_all(&fw1).unwrap();
    ok(
        &fw1,
        &f.path("c1"),
        &["init", "--name", "home", "--schema", "default@1.3.0"],
    );
    ok(
        &fw1,
        &f.path("c1"),
        &["install", f.path("pantry.mem").to_str().unwrap()],
    );
    f
}

/// 02-a and 02-b on a folder workspace with a sealed schema: a missing
/// archive says what to install (not "schema not found"), an archive of
/// other content is named and never mounted in its place, and installing
/// the recorded archive brings the mem back with the roster unchanged.
#[test]
fn a_folder_workspace_with_a_sealed_schema_says_what_to_install() {
    let f = sealed_fixture();
    let (fw1, c1) = (f.path("fw1"), f.path("c1"));
    ok(&fw1, &c1, &["entity", "pantry--flour"]);
    let written = roster(&fw1);
    assert!(written.contains("\"cached-archive\""), "{written}");

    let fw2 = f.path("fw2");
    copy_dir(&fw1, &fw2);
    let c2 = f.path("c2");
    let (code, message) = quarantine_of(&fw2, &c2, "pantry").expect("quarantined");
    assert_eq!(code, "ARCHIVE_NOT_INSTALLED", "{message}");
    assert!(message.contains("memstead install"), "{message}");

    // Other content under the same mem name in this machine's cache.
    let c3 = f.path("c3");
    let fw3 = f.path("fw3");
    copy_dir(&fw1, &fw3);
    let other_ws = f.path("fw-other");
    fs::create_dir_all(&other_ws).unwrap();
    ok(
        &other_ws,
        &c3,
        &["init", "--name", "home", "--schema", "default@1.3.0"],
    );
    ok(
        &other_ws,
        &c3,
        &["install", f.path("pantry-other.mem").to_str().unwrap()],
    );
    let (code, message) = quarantine_of(&fw3, &c3, "pantry").expect("quarantined");
    assert_eq!(code, "ARCHIVE_NOT_INSTALLED", "{message}");
    assert!(
        message.contains("does not match the recorded identity"),
        "{message}"
    );
    assert!(!run(&fw3, &c3, &["entity", "pantry--salt"]).status.success());

    ok(
        &fw2,
        &c2,
        &["install", f.path("pantry.mem").to_str().unwrap()],
    );
    ok(&fw2, &c2, &["entity", "pantry--flour"]);
    assert_eq!(roster(&fw2), written);
}

/// A roster from before identity entries, written on another machine,
/// names that machine's cache file; it heals on this machine: the mem
/// mounts from this machine's cache, and the next write records the
/// identity instead of the foreign path.
#[test]
fn a_roster_naming_another_machines_cache_heals() {
    let f = fixture();
    let (w1, c1) = (f.path("w1"), f.path("c1"));
    let file = fs::read_dir(&c1)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".mem"))
        .unwrap();
    let foreign = format!("/Users/someone-else/Library/Application Support/memstead/mems/{file}");
    let old = serde_json::json!({
        "format": "memstead-mounts-3",
        "mounts": [
            {"mem": "recipes", "schema": "default@1.3.0",
             "storage": {"type": "archive", "path": foreign},
             "capability": "read-only", "lifecycle": "eager", "cross_linkable": false}
        ]
    });
    fs::write(
        w1.join(".memstead/state/mounts.json"),
        serde_json::to_string_pretty(&old).unwrap(),
    )
    .unwrap();
    ok(&w1, &c1, &["entity", "recipes--bread"]);
    ok(
        &w1,
        &c1,
        &["workspace", "allow-create", "--schema", "*", "n*"],
    );
    ok(
        &w1,
        &c1,
        &["mem", "init", "notes", "--schema", "default@1.3.0"],
    );
    let written = roster(&w1);
    assert!(!written.contains("someone-else"), "{written}");
    assert!(written.contains("\"cached-archive\""), "{written}");
}

/// A folder workspace whose mem config still carries a legacy
/// `readMems` registration migrates it at the CLI's first boot, as a
/// mem-repo workspace and the MCP server always did: the entry becomes
/// a workspace mount, the key leaves the config, one warning names the
/// mem, and a second boot is silent.
#[test]
fn a_folder_workspace_migrates_legacy_read_mems_on_the_cli() {
    let f = fixture();
    let c1 = f.path("c1");
    let key = fs::read_dir(&c1)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find_map(|n| {
            n.strip_prefix("recipes-")
                .and_then(|r| r.strip_suffix(".mem"))
                .map(str::to_string)
        })
        .unwrap();

    let fw = f.path("legacy-folder");
    fs::create_dir_all(&fw).unwrap();
    ok(
        &fw,
        &c1,
        &["init", "--name", "home", "--schema", "default@1.3.0"],
    );
    let cfg_path = fw.join(".memstead/config.json");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["readMems"] = serde_json::json!({
        "recipes": {"source": {"type": "local"}, "cacheKey": key}
    });
    fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    let health = ok(&fw, &c1, &["--json", "health"]);
    assert!(
        health.contains("READ_MEMS_MIGRATED_TO_MOUNTS") && health.contains("recipes"),
        "{health}"
    );
    assert!(
        roster(&fw).contains("\"cached-archive\""),
        "{}",
        roster(&fw)
    );
    let cfg_after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert!(
        cfg_after
            .get("readMems")
            .is_none_or(|v| v.as_object().is_some_and(|o| o.is_empty())),
        "{cfg_after}"
    );
    ok(&fw, &c1, &["entity", "recipes--bread"]);
    let again = ok(&fw, &c1, &["--json", "health"]);
    assert!(!again.contains("READ_MEMS_MIGRATED_TO_MOUNTS"), "{again}");
}
