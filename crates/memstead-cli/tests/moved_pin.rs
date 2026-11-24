//! A schema pin that moved where the mem lives.
//!
//! A mem's pin is written into its configuration, which lives with the
//! mem: on the schema-and-config ref of its git directory, or in a
//! folder mem's `.memstead/config.json`. Two rules are held here, on
//! the CLI, through the `memstead-mcp` binary and on the engine:
//!
//! - An engine that is already running serves a mounted mem under a pin
//!   another workspace moved, from its next operation on: no restart, no
//!   reload asked for. Only a moved pin acts; a pin that does not resolve
//!   takes the mem out of service; the roster row is left as it is.
//! - Every reply that names a mem's pin names the configuration's, and
//!   the roster row's only where the configuration states none. A roster
//!   row whose pin is stale, or that carries none, changes no reply.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command as StdCommand, Output, Stdio};

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

const AUTHOR: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "test"),
    ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
    ("GIT_COMMITTER_NAME", "test"),
    ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
];

const OLD: &str = "widgets@0.1.0";
const NEW: &str = "cogs@0.1.0";

fn memstead(ws: &Path) -> Command {
    let mut cmd = Command::cargo_bin("memstead").expect("memstead binary must be built by cargo");
    cmd.current_dir(ws)
        .envs(AUTHOR)
        .env_remove("MEMSTEAD_WORKSPACE")
        .env_remove("MEMSTEAD_IDENTITY");
    cmd
}

fn run(ws: &Path, args: &[&str]) -> Output {
    memstead(ws).args(args).arg("--quiet").output().unwrap()
}

fn printed(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

fn ok(ws: &Path, args: &[&str]) -> String {
    let out = run(ws, args);
    assert!(
        out.status.success(),
        "`memstead {}` failed:\n{}",
        args.join(" "),
        printed(&out)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn json_ok(ws: &Path, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let out = run(ws, &full);
    assert!(
        out.status.success(),
        "`memstead --json {}` refused:\n{}",
        args.join(" "),
        printed(&out)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "`memstead --json {}` printed no JSON ({e}):\n{}",
            args.join(" "),
            printed(&out)
        )
    })
}

fn git(gitdir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .arg("--git-dir")
        .arg(gitdir)
        .args(args)
        .envs(AUTHOR)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// One session with the real `memstead-mcp` binary over stdio.
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(ws: &Path) -> Self {
        let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("memstead-mcp"))
            .arg("--workspace")
            .arg(ws)
            .current_dir(ws)
            .envs(AUTHOR)
            .env_remove("MEMSTEAD_WORKSPACE")
            .env_remove("MEMSTEAD_IDENTITY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("memstead-mcp spawns");
        let stdin = child.stdin.take();
        let reader = BufReader::new(child.stdout.take().unwrap());
        let mut session = Self {
            child,
            stdin,
            reader,
            next_id: 0,
        };
        session.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "moved-pin-test", "version": "0" }
            }),
        );
        session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        session
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).unwrap();
            assert!(read > 0, "memstead-mcp exited before answering {method}");
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && value["id"] == json!(id)
            {
                return value;
            }
        }
    }

    /// The `result` of one `tools/call`.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        assert!(
            reply.get("result").is_some(),
            "{tool} answered no result: {reply}"
        );
        reply["result"].clone()
    }

    /// A call that must answer; its structured content.
    fn answered(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self.call(tool, arguments.clone());
        assert!(
            !result["isError"].as_bool().unwrap_or(false),
            "{tool} {arguments} refused: {result}"
        );
        result["structuredContent"].clone()
    }

    /// A call that must refuse with `code`; its structured content.
    fn refused(&mut self, tool: &str, arguments: Value, code: &str) -> Value {
        let result = self.call(tool, arguments.clone());
        assert!(
            result["isError"].as_bool().unwrap_or(false),
            "{tool} {arguments} answered: {result}"
        );
        assert_eq!(result["structuredContent"]["code"], code, "{result}");
        result["structuredContent"].clone()
    }

    /// The text of a call that must answer (the markdown replies).
    fn text(&mut self, tool: &str, arguments: Value) -> String {
        let result = self.call(tool, arguments.clone());
        assert!(
            !result["isError"].as_bool().unwrap_or(false),
            "{tool} {arguments} refused: {result}"
        );
        result["content"][0]["text"].as_str().unwrap().to_string()
    }

    /// The pin an entity read names.
    fn pin_read(&mut self, id: &str) -> Value {
        self.answered("memstead_entity", json!({ "id": id }))["_mem_schema"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A scaffolded schema package `<pkg>/<name>` that declares `types`,
/// each a copy of the scaffold's `note` (required section `summary`).
fn scaffold(pkg: &Path, name: &str, types: &[&str]) -> String {
    fs::create_dir_all(pkg).unwrap();
    ok(pkg, &["schema", "new", name]);
    let dir = pkg.join(name);
    let manifest = fs::read_to_string(dir.join("schema.yaml")).unwrap();
    assert!(manifest.contains("types:\n  - note\n"), "{manifest}");
    let listed: String = types.iter().map(|t| format!("  - {t}\n")).collect();
    fs::write(
        dir.join("schema.yaml"),
        manifest.replace("types:\n  - note\n", &format!("types:\n{listed}")),
    )
    .unwrap();
    let note = fs::read_to_string(dir.join("types/note.yaml")).unwrap();
    assert!(note.contains("\nname: note\n"), "{note}");
    for other in types.iter().filter(|t| **t != "note") {
        fs::write(
            dir.join(format!("types/{other}.yaml")),
            note.replace("\nname: note\n", &format!("\nname: {other}\n")),
        )
        .unwrap();
    }
    if !types.contains(&"note") {
        fs::remove_file(dir.join("types/note.yaml")).unwrap();
    }
    dir.to_str().unwrap().to_string()
}

fn init_store(store: &Path) -> PathBuf {
    fs::create_dir_all(store).unwrap();
    ok(store, &["mem-repo", "init", "."]);
    ok(store, &["workspace", "allow-create", "--schema", "*", "*"]);
    store.join("mem-repo/.git").canonicalize().unwrap()
}

/// Mount `mem` of the store's git directory into `view`, as the first
/// mount of a new workspace when `view` holds none yet.
fn mount(view: &Path, gitdir: &Path, mem: &str, capability: &str) {
    let fresh = !view.join(".memstead").exists();
    fs::create_dir_all(view).unwrap();
    let mut args = vec![
        "mem",
        "mount",
        mem,
        "--branch",
        mem,
        "--gitdir",
        gitdir.to_str().unwrap(),
        "--capability",
        capability,
        "--operator-mode",
    ];
    if fresh {
        args.push("--new-workspace");
    }
    ok(view, &args);
}

fn create(ws: &Path, mem: &str, title: &str, entity_type: &str) {
    ok(
        ws,
        &[
            "create",
            "--mem",
            mem,
            "--title",
            title,
            "--type",
            entity_type,
            "--section",
            "summary=Hello.",
        ],
    );
}

/// The arguments of an MCP create of one scaffold-shaped entity.
fn entity(mem: &str, title: &str, entity_type: &str) -> Value {
    json!({
        "mem": mem,
        "title": title,
        "entity_type": entity_type,
        "sections": { "summary": "Hello." }
    })
}

/// Move a mem's pin in the workspace it belongs to; the move must be a
/// settled switch, not the start of a migration.
fn move_pin(ws: &Path, mem: &str, pin: &str) {
    let moved = json_ok(ws, &["mem", "set-schema", mem, pin]);
    assert_eq!(moved["outcome"], "switched", "{moved}");
}

fn roster(ws: &Path) -> String {
    fs::read_to_string(ws.join(".memstead/state/mounts.json")).unwrap()
}

/// Rewrite the `schema` of every row of a workspace's roster: to a pin,
/// or away.
fn set_roster_pin(ws: &Path, pin: Option<&str>) {
    let path = ws.join(".memstead/state/mounts.json");
    let mut value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    for row in value["mounts"].as_array_mut().unwrap() {
        let row = row.as_object_mut().unwrap();
        match pin {
            Some(pin) => row.insert("schema".to_string(), json!(pin)),
            None => row.remove("schema"),
        };
    }
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

struct Fixture {
    _root: TempDir,
    store: PathBuf,
    view: PathBuf,
    pkg: PathBuf,
    gitdir: PathBuf,
}

/// A store with the schemas `widgets` (types `note`, `part`) and `cogs`
/// (types `note`, `gear`) installed, each of `mems` pinned to `pin` and
/// holding the note `One`, and a view that mounts every one of them
/// with `capability`. A note is integral under either schema, so a pin
/// switch between the two settles at once.
fn store_and_view(capability: &str, mems: &[&str], pin: &str) -> Fixture {
    let root = TempDir::new().unwrap();
    let store = root.path().join("store");
    let view = root.path().join("view");
    let pkg = root.path().join("pkg");
    let gitdir = init_store(&store);
    for (name, types) in [("widgets", ["note", "part"]), ("cogs", ["note", "gear"])] {
        let package = scaffold(&pkg, name, &types);
        ok(&store, &["schema", "install", &package]);
    }
    for mem in mems {
        ok(&store, &["mem", "init", mem, "--schema", pin]);
        create(&store, mem, "One", "note");
        mount(&view, &gitdir, mem, capability);
    }
    Fixture {
        _root: root,
        store,
        view,
        pkg,
        gitdir,
    }
}

/// A reply raised no notice about `mem`: no reload warning, no roster
/// warning, no change notice.
fn assert_no_notice(reply: &Value) {
    let text = reply.to_string();
    for marker in ["MEM_RELOADED", "MEM_ROSTER_CHANGED", "mem_changed"] {
        assert!(!text.contains(marker), "{marker} in {reply}");
    }
}

// ---------------------------------------------------------------------
// A running engine follows a moved pin
// ---------------------------------------------------------------------

/// The central case: a writable mount, the pin moves to a schema that
/// is installed in the store and that no mem the view mounts pinned
/// before, so the view has to take it from the store's registry.
#[test]
fn a_running_view_follows_a_pin_moved_to_a_schema_no_mounted_mem_pinned() {
    let fx = store_and_view("write", &["kept"], OLD);
    let mut mcp = Mcp::start(&fx.view);
    assert_eq!(mcp.pin_read("kept--one"), OLD);
    mcp.refused(
        "memstead_create",
        entity("kept", "Too Early", "gear"),
        "UNKNOWN_ENTITY_TYPE",
    );
    mcp.refused(
        "memstead_schema",
        json!({ "name": NEW }),
        "SCHEMA_NOT_FOUND",
    );
    let roster_before = roster(&fx.view);

    move_pin(&fx.store, "kept", NEW);

    // The next call, with no reload asked for: a type only the new
    // schema declares.
    let made = mcp.answered("memstead_create", entity("kept", "A Gear", "gear"));
    assert_eq!(made["id"], "kept--a-gear", "{made}");
    assert_eq!(made["_mem_schema"], NEW, "{made}");
    // A type only the old schema declares is refused, naming the new.
    let refused = mcp.refused(
        "memstead_create",
        entity("kept", "A Part", "part"),
        "UNKNOWN_ENTITY_TYPE",
    );
    assert_eq!(refused["details"]["schema_ref"], NEW, "{refused}");
    assert_eq!(mcp.pin_read("kept--one"), NEW);
    assert_eq!(mcp.pin_read("kept--a-gear"), NEW);

    // The roster row is not rewritten: its pin stays the view's own
    // statement.
    assert_eq!(roster(&fx.view), roster_before);
    assert!(roster_before.contains(&format!("\"schema\": \"{OLD}\"")));
    // The write landed where the mem lives.
    let seen = ok(&fx.store, &["entity", "kept--a-gear"]);
    assert!(seen.contains("A Gear"), "{seen}");
}

/// The pin moves between two generations of a built-in schema. No two
/// generations of a built-in schema each declare a type the other
/// lacks, so the two halves take two moves: `ingest@0.5.0` declares
/// `negative_finding`, `ingest@0.4.0` does not.
#[test]
fn a_running_view_follows_a_pin_moved_to_another_generation_of_a_built_in_schema() {
    let root = TempDir::new().unwrap();
    let store = root.path().join("store");
    let view = root.path().join("view");
    let gitdir = init_store(&store);
    ok(&store, &["mem", "init", "kept", "--schema", "ingest@0.5.0"]);
    ok(
        &store,
        &[
            "create",
            "--mem",
            "kept",
            "--title",
            "One",
            "--type",
            "coverage_gap",
            "--section",
            "area=a",
            "--section",
            "evidence=e",
        ],
    );
    mount(&view, &gitdir, "kept", "write");
    let finding = |title: &str| {
        json!({
            "mem": "kept",
            "title": title,
            "entity_type": "negative_finding",
            "sections": { "sought": "s", "search_path": "p", "finding": "f" }
        })
    };
    let mut mcp = Mcp::start(&view);
    assert_eq!(mcp.pin_read("kept--one"), "ingest@0.5.0");

    // Down a generation: the type only the old generation declares is
    // refused by the very next call, naming the new one.
    move_pin(&store, "kept", "ingest@0.4.0");
    let refused = mcp.refused("memstead_create", finding("Gone"), "UNKNOWN_ENTITY_TYPE");
    assert_eq!(
        refused["details"]["schema_ref"], "ingest@0.4.0",
        "{refused}"
    );
    assert_eq!(mcp.pin_read("kept--one"), "ingest@0.4.0");

    // Up again: the type only the new generation declares is accepted.
    move_pin(&store, "kept", "ingest@0.5.0");
    let made = mcp.answered("memstead_create", finding("Nothing There"));
    assert_eq!(made["_mem_schema"], "ingest@0.5.0", "{made}");
    assert_eq!(mcp.pin_read("kept--one"), "ingest@0.5.0");
}

/// A read-only mount follows on its reads: the entity read names the
/// new pin and the schema tool asked by mem returns the new schema.
#[test]
fn a_read_only_view_follows_a_moved_pin_on_its_reads() {
    let fx = store_and_view("read-only", &["kept"], OLD);
    let mut mcp = Mcp::start(&fx.view);
    assert_eq!(mcp.pin_read("kept--one"), OLD);
    let before = mcp.answered("memstead_schema", json!({ "mem": "kept" }));
    assert_eq!(before["ref"], OLD, "{before}");

    move_pin(&fx.store, "kept", NEW);

    assert_eq!(mcp.pin_read("kept--one"), NEW);
    let after = mcp.answered(
        "memstead_schema",
        json!({ "mem": "kept", "verbosity": "full" }),
    );
    assert_eq!(after["ref"], NEW, "{after}");
    let body = after.to_string();
    assert!(
        body.contains("gear") && !body.contains("\"part\""),
        "{body}"
    );
    // Still read-only, and nothing was laid in the view's roster.
    mcp.refused(
        "memstead_create",
        entity("kept", "A Gear", "gear"),
        "READ_ONLY_MOUNT",
    );
    assert!(!roster(&fx.view).contains("cogs"));
}

/// Two mems of the store are mounted and both pins move between two
/// calls of the view.
#[test]
fn two_mounted_mems_follow_when_both_pins_move_between_two_calls() {
    let fx = store_and_view("write", &["kept", "other"], OLD);
    let mut mcp = Mcp::start(&fx.view);
    assert_eq!(mcp.pin_read("kept--one"), OLD);
    assert_eq!(mcp.pin_read("other--one"), OLD);

    move_pin(&fx.store, "kept", NEW);
    move_pin(&fx.store, "other", NEW);

    for mem in ["kept", "other"] {
        let made = mcp.answered("memstead_create", entity(mem, "A Gear", "gear"));
        assert_eq!(made["_mem_schema"], NEW, "{made}");
        let refused = mcp.refused(
            "memstead_create",
            entity(mem, "A Part", "part"),
            "UNKNOWN_ENTITY_TYPE",
        );
        assert_eq!(refused["details"]["schema_ref"], NEW, "{refused}");
        assert_eq!(mcp.pin_read(&format!("{mem}--one")), NEW);
    }
}

/// A folder mem can be mounted by a second workspace (`mem init
/// --storage folder --location <the folder> --reattach`), and its pin
/// is moved by the workspace it belongs to. A folder pin move writes
/// the folder's configuration and no ledger line, so the mem's head
/// does not move with it.
#[test]
fn a_folder_mem_a_second_workspace_mounts_follows_the_pin_its_workspace_moved() {
    let root = TempDir::new().unwrap();
    let owner = root.path().join("owner");
    let second = root.path().join("second");
    for ws in [&owner, &second] {
        init_store(ws);
    }
    ok(
        &owner,
        &[
            "mem",
            "init",
            "kept",
            "--storage",
            "folder",
            "--schema",
            "ingest@0.4.0",
        ],
    );
    ok(
        &owner,
        &[
            "create",
            "--mem",
            "kept",
            "--title",
            "One",
            "--type",
            "coverage_gap",
            "--section",
            "area=a",
            "--section",
            "evidence=e",
        ],
    );
    let folder = owner.join("kept");
    ok(
        &second,
        &[
            "mem",
            "init",
            "kept",
            "--storage",
            "folder",
            "--location",
            folder.to_str().unwrap(),
            "--operator-mode",
            "--reattach",
        ],
    );
    let finding = |title: &str| {
        json!({
            "mem": "kept",
            "title": title,
            "entity_type": "negative_finding",
            "sections": { "sought": "s", "search_path": "p", "finding": "f" }
        })
    };
    let mut mcp = Mcp::start(&second);
    assert_eq!(mcp.pin_read("kept--one"), "ingest@0.4.0");
    mcp.refused(
        "memstead_create",
        finding("Too Early"),
        "UNKNOWN_ENTITY_TYPE",
    );
    let ledger = fs::read_to_string(folder.join(".memstead/changes.jsonl")).unwrap();

    move_pin(&owner, "kept", "ingest@0.5.0");
    assert_eq!(
        fs::read_to_string(folder.join(".memstead/changes.jsonl")).unwrap(),
        ledger,
        "a folder pin move appends no ledger line"
    );

    let made = mcp.answered("memstead_create", finding("Nothing There"));
    assert_eq!(made["_mem_schema"], "ingest@0.5.0", "{made}");
    assert_eq!(mcp.pin_read("kept--one"), "ingest@0.5.0");

    // And down again, once the mem holds nothing the old generation
    // lacks: the type is refused, naming the generation moved to.
    ok(&owner, &["delete", "kept--nothing-there"]);
    move_pin(&owner, "kept", "ingest@0.4.0");
    let refused = mcp.refused("memstead_create", finding("Gone"), "UNKNOWN_ENTITY_TYPE");
    assert_eq!(
        refused["details"]["schema_ref"], "ingest@0.4.0",
        "{refused}"
    );
    assert_eq!(mcp.pin_read("kept--one"), "ingest@0.4.0");
}

// ---------------------------------------------------------------------
// Only a moved pin acts
// ---------------------------------------------------------------------

/// One act in the store: what it is, and the act.
type Act<'a> = (&'static str, Box<dyn Fn() + 'a>);

/// The acts of the store that change the registry ref and move no pin
/// of a mem the view mounts.
fn acts_that_move_no_pin(fx: &Fixture) -> Vec<Act<'_>> {
    vec![
        (
            "the store sets the version of the mem",
            Box::new(|| {
                ok(&fx.store, &["mem", "set-version", "kept", "0.2.0"]);
            }),
        ),
        (
            "the store sets the description of the mem",
            Box::new(|| {
                ok(&fx.store, &["mem", "set-description", "kept", "Kept."]);
            }),
        ),
        (
            "the store installs a schema",
            Box::new(|| {
                let package = scaffold(&fx.pkg, "gadgets", &["note"]);
                ok(&fx.store, &["schema", "install", &package]);
            }),
        ),
        (
            "the store moves the pin of a mem the view does not mount",
            Box::new(|| move_pin(&fx.store, "other", NEW)),
        ),
    ]
}

/// A store with `kept` (mounted in the view) and `other` (not mounted).
fn store_with_an_unmounted_mem() -> Fixture {
    let fx = store_and_view("write", &["kept"], OLD);
    ok(&fx.store, &["mem", "init", "other", "--schema", OLD]);
    fx
}

/// On the engine: after each such act the next operation's check hands
/// out no warning, the store's generation stands (nothing was parsed
/// again), the bound schema and the cached configuration are the ones
/// the engine had, and nothing is quarantined. A pin move then does
/// move all of it, which shows the observations bite.
#[test]
fn store_acts_that_move_no_pin_neither_reload_nor_remount_the_mem() {
    let fx = store_with_an_unmounted_mem();
    let mut engine = memstead_git_branch::workspace_store::engine_from_workspace_root(&fx.view)
        .expect("the view boots");
    assert!(engine.reload_if_stale(None).is_empty());
    let generation = engine.store().generation();
    let bound = engine.resolved_schema("kept").unwrap().id();
    let config = serde_json::to_value(engine.mem_config_for("kept").unwrap()).unwrap();
    assert_eq!(config["version"], "0.1.0", "{config}");
    let registry_ref = || git(&fx.gitdir, &["rev-parse", "refs/heads/__MEMSTEAD"]);

    for (what, act) in acts_that_move_no_pin(&fx) {
        let before = registry_ref();
        act();
        assert_ne!(registry_ref(), before, "{what}: the registry ref moved");
        for scope in [None, Some("kept")] {
            let warnings = engine.reload_if_stale(scope);
            assert!(warnings.is_empty(), "{what}: {warnings:?}");
        }
        assert_eq!(engine.store().generation(), generation, "{what}");
        assert_eq!(
            engine.resolved_schema("kept").unwrap().id(),
            bound,
            "{what}"
        );
        assert_eq!(
            serde_json::to_value(engine.mem_config_for("kept").unwrap()).unwrap(),
            config,
            "{what}: the cached configuration is the one the engine had"
        );
        assert!(engine.quarantine_reason("kept").is_none(), "{what}");
    }

    // The contrast: a moved pin binds the new schema and parses the mem
    // again.
    move_pin(&fx.store, "kept", NEW);
    engine.reload_if_stale(Some("kept"));
    let (name, version) = engine.resolved_schema("kept").unwrap().id();
    assert_eq!(format!("{name}@{version}"), NEW);
    assert_ne!(engine.store().generation(), generation);
    assert_eq!(engine.schema_pin("kept").unwrap().to_string(), NEW);
    // The roster row's pin is still the cross-check it was.
    assert_eq!(
        engine
            .mount("kept")
            .unwrap()
            .schema
            .as_ref()
            .unwrap()
            .to_string(),
        OLD
    );
}

/// The same over the wire: a caller of the view sees no notice, the pin
/// it had, the version the engine cached, and a reload report without a
/// change.
#[test]
fn store_acts_that_move_no_pin_change_nothing_a_caller_of_the_view_sees() {
    let fx = store_with_an_unmounted_mem();
    let mut mcp = Mcp::start(&fx.view);
    assert_eq!(mcp.pin_read("kept--one"), OLD);
    let cached_version = |mcp: &mut Mcp| {
        let overview = mcp.text("memstead_overview", json!({}));
        assert!(
            overview.contains(&format!("- **Schema:** {OLD}")),
            "{overview}"
        );
        overview.contains("- **Version:** 0.1.0")
    };
    assert!(cached_version(&mut mcp));

    for (what, act) in acts_that_move_no_pin(&fx) {
        act();
        let read = mcp.answered("memstead_entity", json!({ "id": "kept--one" }));
        assert_eq!(read["_mem_schema"], OLD, "{what}: {read}");
        assert_no_notice(&read);
        let made = mcp.answered(
            "memstead_create",
            entity("kept", &format!("Part after {}", what.len()), "part"),
        );
        assert_eq!(made["_mem_schema"], OLD, "{what}: {made}");
        assert_no_notice(&made);
        assert!(
            cached_version(&mut mcp),
            "{what}: the engine reloaded the mem's configuration"
        );
    }

    // A reload asked for now reports no change, and the pin stands.
    let reloaded = mcp.answered("memstead_reload", json!({ "mem": "kept" }));
    let report = &reloaded["reports"][0];
    assert_eq!(report["changed_entity_ids"], json!([]), "{reloaded}");
    assert_eq!(report["head_before"], report["head_after"], "{reloaded}");
    assert_no_notice(&reloaded);
    assert_eq!(mcp.pin_read("kept--one"), OLD);
    mcp.answered("memstead_create", entity("kept", "Still A Part", "part"));
}

/// A migration the view started itself (its own roster row carries the
/// target) is not overridden by what the store does to the mem's
/// configuration: writes stay validated against the target. The replies
/// name the settled pin and the target beside it.
#[test]
fn a_migration_the_view_started_keeps_its_target_when_the_store_touches_the_configuration() {
    let fx = store_and_view("write", &["kept"], OLD);
    // A view switches only to a schema it resolves, and from the
    // store's registry it takes what its mounts pin: a second mounted
    // mem pins the target.
    ok(&fx.store, &["mem", "init", "other", "--schema", NEW]);
    mount(&fx.view, &fx.gitdir, "other", "read-only");
    // A third schema both hold, which the note and the part are
    // integral under: the store's later settled move.
    let package = scaffold(&fx.pkg, "sprockets", &["note", "part"]);
    ok(&fx.store, &["schema", "install", &package]);
    create(&fx.store, "kept", "A Part", "part");

    let mut mcp = Mcp::start(&fx.view);
    let started = mcp.answered(
        "memstead_mem_set_schema",
        json!({ "mem": "kept", "schema": NEW }),
    );
    assert_eq!(started["outcome"], "migration_started", "{started}");
    assert_eq!(started["schema_pin"], OLD, "{started}");
    assert_eq!(started["migration_target"], NEW, "{started}");
    assert!(roster(&fx.view).contains("\"migration_target\": \"cogs@0.1.0\""));

    let under_the_target = |mcp: &mut Mcp, title: &str, settled: &str| {
        let made = mcp.answered("memstead_create", entity("kept", title, "gear"));
        assert_eq!(made["_mem_schema"], settled, "{made}");
        let refused = mcp.refused(
            "memstead_create",
            entity("kept", "Another Part", "part"),
            "UNKNOWN_ENTITY_TYPE",
        );
        assert_eq!(refused["details"]["schema_ref"], NEW, "{refused}");
        let health = mcp.answered("memstead_health", json!({ "mem": "kept" }));
        assert_eq!(
            health["mem_schemas"],
            json!([{ "mem": "kept", "schema": settled, "writable": true, "migration_target": NEW }]),
            "{health}"
        );
        assert_eq!(health["_mem_schema"], settled, "{health}");
    };
    under_the_target(&mut mcp, "First Gear", OLD);

    // The store touches the configuration without moving the pin.
    ok(&fx.store, &["mem", "set-version", "kept", "0.3.0"]);
    ok(&fx.store, &["mem", "set-description", "kept", "Kept."]);
    under_the_target(&mut mcp, "Second Gear", OLD);

    // The store has no part in the view's migration: its own pin is the
    // settled one, and it may move it. The view's target still stands,
    // and the settled pin the replies name is the configuration's.
    ok(&fx.store, &["delete", "kept--first-gear"]);
    ok(&fx.store, &["delete", "kept--second-gear"]);
    move_pin(&fx.store, "kept", "sprockets@0.1.0");
    under_the_target(&mut mcp, "Third Gear", "sprockets@0.1.0");
}

/// A pin moved to a schema the view cannot resolve: the store holds it
/// in its own schema folder, which no other workspace reads. The mem
/// goes out of service in the view under the reason a boot gives, no
/// write is validated against the old schema, and once the schema is
/// resolvable a reload brings the mem back under the new pin.
#[test]
fn a_pin_the_view_cannot_resolve_takes_the_mem_out_of_service_until_a_reload_resolves_it() {
    for reload in [json!({ "mem": "kept" }), json!({ "full": true })] {
        let fx = store_and_view("write", &["kept"], OLD);
        let package = scaffold(&fx.pkg, "levers", &["note", "lever"]);
        let held_by_the_store_alone = fx.store.join(".memstead/schemas/levers@0.1.0");
        fs::create_dir_all(&held_by_the_store_alone).unwrap();
        for file in ["schema.yaml", "types/note.yaml", "types/lever.yaml"] {
            let to = held_by_the_store_alone.join(file);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(Path::new(&package).join(file), to).unwrap();
        }

        let mut mcp = Mcp::start(&fx.view);
        assert_eq!(mcp.pin_read("kept--one"), OLD);
        move_pin(&fx.store, "kept", "levers@0.1.0");
        let tip = git(&fx.gitdir, &["rev-parse", "refs/heads/kept"]);

        // Out of service from the next call on: a type the old schema
        // declares, a type both declare, and a read.
        for (title, entity_type) in [("A Part", "part"), ("A Note", "note")] {
            let refused = mcp.refused(
                "memstead_create",
                entity("kept", title, entity_type),
                "MEM_QUARANTINED",
            );
            assert_eq!(
                refused["details"]["reason_code"], "SCHEMA_NOT_FOUND",
                "{refused}"
            );
        }
        mcp.refused(
            "memstead_entity",
            json!({ "id": "kept--one" }),
            "MEM_QUARANTINED",
        );
        assert_eq!(git(&fx.gitdir, &["rev-parse", "refs/heads/kept"]), tip);

        // The reason is the one a boot gives for the same pin.
        let health = mcp.answered("memstead_health", json!({}));
        let running = &health["quarantined"][0];
        let booted = json_ok(&fx.view, &["mem", "list"]);
        let at_boot = &booted["quarantined"][0];
        assert_eq!(running["mem"], "kept", "{health}");
        assert_eq!(running["reason_code"], "SCHEMA_NOT_FOUND", "{health}");
        assert_eq!(running["reason_code"], at_boot["reason_code"], "{booted}");
        assert_eq!(running["reason_message"], at_boot["reason"], "{booted}");

        // A reload while the schema is still unresolvable changes nothing.
        mcp.call("memstead_reload", reload.clone());
        mcp.refused(
            "memstead_create",
            entity("kept", "A Note", "note"),
            "MEM_QUARANTINED",
        );

        // The schema becomes resolvable: installed in the store's
        // registry, from which the view takes what its mounts pin.
        ok(&fx.store, &["schema", "install", &package]);
        mcp.answered("memstead_reload", reload.clone());
        let made = mcp.answered("memstead_create", entity("kept", "A Lever", "lever"));
        assert_eq!(made["_mem_schema"], "levers@0.1.0", "{reload}: {made}");
        mcp.refused(
            "memstead_create",
            entity("kept", "A Part", "part"),
            "UNKNOWN_ENTITY_TYPE",
        );
        assert_eq!(mcp.pin_read("kept--one"), "levers@0.1.0");
    }
}

/// In the workspace that moves a pin, a server already running follows
/// through its own roster row, which the move rewrites: the mount is
/// taken down and brought up under the new pin, as before.
#[test]
fn a_server_in_the_workspace_that_moves_the_pin_follows_through_its_roster() {
    let remounted = |made: &Value, mem: &str| {
        let warning = made["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["code"] == "MEM_ROSTER_CHANGED")
            .unwrap_or_else(|| panic!("no roster warning in {made}"));
        assert_eq!(warning["details"]["added"], json!([mem]), "{made}");
        assert_eq!(warning["details"]["removed"], json!([mem]), "{made}");
    };

    // A mem-repo workspace.
    let fx = store_and_view("write", &["kept"], OLD);
    let mut mcp = Mcp::start(&fx.store);
    assert_eq!(mcp.pin_read("kept--one"), OLD);
    move_pin(&fx.store, "kept", NEW);
    assert!(roster(&fx.store).contains(&format!("\"schema\": \"{NEW}\"")));
    let made = mcp.answered("memstead_create", entity("kept", "A Gear", "gear"));
    assert_eq!(made["_mem_schema"], NEW, "{made}");
    remounted(&made, "kept");
    assert_eq!(mcp.pin_read("kept--one"), NEW);

    // A folder workspace.
    let root = TempDir::new().unwrap();
    let ws = root.path().join("solo");
    fs::create_dir_all(&ws).unwrap();
    ok(
        &ws,
        &["init", ".", "--name", "solo", "--schema", "ingest@0.4.0"],
    );
    ok(
        &ws,
        &[
            "create",
            "--title",
            "One",
            "--type",
            "coverage_gap",
            "--section",
            "area=a",
            "--section",
            "evidence=e",
        ],
    );
    let mut mcp = Mcp::start(&ws);
    assert_eq!(mcp.pin_read("solo--one"), "ingest@0.4.0");
    move_pin(&ws, "solo", "ingest@0.5.0");
    let made = mcp.answered(
        "memstead_create",
        json!({
            "mem": "solo",
            "title": "Nothing There",
            "entity_type": "negative_finding",
            "sections": { "sought": "s", "search_path": "p", "finding": "f" }
        }),
    );
    assert_eq!(made["_mem_schema"], "ingest@0.5.0", "{made}");
    remounted(&made, "solo");
}

// ---------------------------------------------------------------------
// Every reply names the configuration's pin
// ---------------------------------------------------------------------

/// What the view's roster row says beside a configuration that pins
/// `cogs@0.1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    /// The same pin.
    Agrees,
    /// An older pin: the store moved it and the view booted afterwards.
    Older,
    /// No pin at all.
    None,
}

/// A view in the given state over a store whose mem `kept` is pinned to
/// `cogs@0.1.0` and holds the notes `One`, `Two` and `Three`.
fn view_whose_row(row: Row) -> Fixture {
    let fx = match row {
        Row::Older => {
            let fx = store_and_view("write", &["kept"], OLD);
            move_pin(&fx.store, "kept", NEW);
            fx
        }
        Row::Agrees | Row::None => store_and_view("write", &["kept"], NEW),
    };
    create(&fx.store, "kept", "Two", "note");
    create(&fx.store, "kept", "Three", "note");
    if row == Row::None {
        set_roster_pin(&fx.view, None);
    }
    let text = roster(&fx.view);
    match row {
        Row::Agrees => assert!(text.contains(&format!("\"schema\": \"{NEW}\"")), "{text}"),
        Row::Older => assert!(text.contains(&format!("\"schema\": \"{OLD}\"")), "{text}"),
        Row::None => assert!(!text.contains("\"schema\""), "{text}"),
    }
    fx
}

/// Every reply of the list, in every state of the roster row. The
/// engine boots after the state is in place, so this is the reading
/// alone, with no pin moving under a running engine.
#[test]
fn every_reply_names_the_configurations_pin_whatever_the_roster_row_says() {
    for row in [Row::Agrees, Row::Older, Row::None] {
        let fx = view_whose_row(row);
        let mut mcp = Mcp::start(&fx.view);

        // The schema tool asked by mem: the configuration's pin and
        // that schema's body.
        let schema = mcp.answered(
            "memstead_schema",
            json!({ "mem": "kept", "verbosity": "full" }),
        );
        assert_eq!(schema["ref"], NEW, "{row:?}: {schema}");
        assert_eq!(schema["used_by"], json!(["kept"]), "{row:?}: {schema}");
        let body = schema.to_string();
        assert!(
            body.contains("gear") && !body.contains("\"part\""),
            "{row:?}"
        );

        // Entity reads and every mutation reply.
        let read = mcp.answered("memstead_entity", json!({ "id": "kept--one" }));
        assert_eq!(read["_mem_schema"], NEW, "{row:?}: {read}");
        let hash = |mcp: &mut Mcp, id: &str| {
            mcp.answered("memstead_entity", json!({ "id": id }))["_hash"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let made = mcp.answered("memstead_create", entity("kept", "Fresh", "gear"));
        assert_eq!(made["_mem_schema"], NEW, "{row:?}: {made}");
        let one = hash(&mut mcp, "kept--one");
        let updated = mcp.answered(
            "memstead_update",
            json!({ "id": "kept--one", "sections": { "summary": "Changed." }, "expected_hash": one }),
        );
        assert_eq!(updated["_mem_schema"], NEW, "{row:?}: {updated}");
        let related = mcp.answered(
            "memstead_relate",
            json!({ "relations": [{ "from": "kept--one", "to": "kept--two", "rel_type": "RELATES_TO" }] }),
        );
        assert_eq!(related["_mem_schema"], NEW, "{row:?}: {related}");
        let three = hash(&mut mcp, "kept--three");
        let renamed = mcp.answered(
            "memstead_rename",
            json!({ "id": "kept--three", "new_title": "Third", "expected_hash": three }),
        );
        assert_eq!(renamed["_mem_schema"], NEW, "{row:?}: {renamed}");
        let third = hash(&mut mcp, "kept--third");
        let retyped = mcp.answered(
            "memstead_retype",
            json!({ "id": "kept--third", "target_type": "gear", "expected_hash": third }),
        );
        assert_eq!(retyped["_mem_schema"], NEW, "{row:?}: {retyped}");
        let third = hash(&mut mcp, "kept--third");
        let deleted = mcp.answered(
            "memstead_delete",
            json!({ "id": "kept--third", "expected_hash": third }),
        );
        assert_eq!(deleted["_mem_schema"], NEW, "{row:?}: {deleted}");

        // The health read: the roster of pins, the anchor of a read
        // scoped to the mem, and the per-schema counts.
        let health = mcp.answered("memstead_health", json!({ "mem": "kept" }));
        assert_eq!(
            health["mem_schemas"],
            json!([{ "mem": "kept", "schema": NEW, "writable": true }]),
            "{row:?}: {health}"
        );
        assert_eq!(health["_mem_schema"], NEW, "{row:?}: {health}");
        let by_schema = health["summary"]["communities_by_schema"]
            .as_object()
            .unwrap();
        assert_eq!(
            by_schema.keys().collect::<Vec<_>>(),
            vec![NEW],
            "{row:?}: {health}"
        );

        // The overview's schema line, and its anchor when scoped.
        let overview = mcp.text("memstead_overview", json!({}));
        assert!(
            overview.contains(&format!("- **Schema:** {NEW}"))
                && overview.contains(&format!("### {NEW}")),
            "{row:?}: {overview}"
        );
        assert!(
            !overview.contains(&format!("- **Schema:** {OLD}")),
            "{row:?}"
        );
        let scoped = mcp.text("memstead_overview", json!({ "mem": "kept" }));
        assert!(
            scoped.contains(&format!("_mem_schema: {NEW}")),
            "{row:?}: {scoped}"
        );
        drop(mcp);

        // The CLI.
        let listed = json_ok(&fx.view, &["mem", "list"]);
        assert_eq!(listed["mems"][0]["schema_ref"], NEW, "{row:?}: {listed}");
        let listed = ok(&fx.view, &["mem", "list"]);
        assert!(
            listed.contains(&format!("schema `{NEW}`")),
            "{row:?}: {listed}"
        );
        let overview = ok(&fx.view, &["overview"]);
        assert!(
            overview.contains(&format!("- **Schema:** {NEW}")),
            "{row:?}: {overview}"
        );
        let health = json_ok(&fx.view, &["health"]);
        assert_eq!(health["mem_schemas"][0]["schema"], NEW, "{row:?}: {health}");
        let dumped = json_ok(&fx.view, &["workspace", "dump"]);
        assert!(
            dumped
                .to_string()
                .contains(&format!("\"schema_ref\":\"{NEW}\"")),
            "{row:?}: {dumped}"
        );
        // `entity --json` carries no pin field; it names no stale one.
        let entity = json_ok(&fx.view, &["entity", "kept--one"]);
        assert!(entity.get("_mem_schema").is_none(), "{entity}");
        assert!(!entity.to_string().contains(OLD), "{row:?}: {entity}");

        // A boot beside an older row still raises the cross-check, with
        // both values; beside an agreeing or an absent row it has
        // nothing to raise.
        let mismatch: Vec<Value> = health["warnings"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter(|w| w["code"] == "SCHEMA_PIN_MISMATCH")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if row == Row::Older {
            assert_eq!(mismatch.len(), 1, "{health}");
            assert_eq!(
                mismatch[0]["details"],
                json!({ "mem": "kept", "config_pin": NEW, "mount_pin": OLD }),
                "{health}"
            );
            assert!(roster(&fx.view).contains(&format!("\"schema\": \"{OLD}\"")));
        } else {
            assert!(mismatch.is_empty(), "{row:?}: {health}");
        }
    }
}

/// With a roster row that carries no pin the schema tool asked by mem
/// returns the schema, in a view and in a plain mem-repo workspace,
/// where it used to refuse `SCHEMA_NOT_FOUND` for an empty name.
#[test]
fn the_schema_tool_asked_by_mem_answers_where_the_roster_row_carries_no_pin() {
    let fx = store_and_view("write", &["kept"], NEW);
    for ws in [&fx.view, &fx.store] {
        set_roster_pin(ws, None);
        let mut mcp = Mcp::start(ws);
        let schema = mcp.answered("memstead_schema", json!({ "mem": "kept" }));
        assert_eq!(schema["ref"], NEW, "{schema}");
        assert_eq!(mcp.pin_read("kept--one"), NEW);
    }
}

/// A roster row whose pin is stale, or that carries none, changes no
/// read reply: one view, the same store, the row rewritten between
/// boots. Beside an absent row every reply is the agreeing state's;
/// beside an older one all but the replies that carry the boot's
/// cross-check warning are.
#[test]
fn a_stale_or_absent_roster_pin_changes_no_read_reply() {
    let fx = store_and_view("write", &["kept"], NEW);
    let replies = |fx: &Fixture| -> Vec<(&'static str, String)> {
        let mut mcp = Mcp::start(&fx.view);
        let mut out = vec![
            (
                "memstead_schema by mem",
                mcp.answered("memstead_schema", json!({ "mem": "kept" }))
                    .to_string(),
            ),
            (
                "memstead_entity",
                mcp.answered(
                    "memstead_entity",
                    json!({ "id": "kept--one", "include_relations": true }),
                )
                .to_string(),
            ),
            (
                "memstead_search",
                mcp.call("memstead_search", json!({ "mem": "kept", "limit": 5 }))
                    .to_string(),
            ),
            (
                "memstead_changes_since",
                mcp.call(
                    "memstead_changes_since",
                    json!({ "mem": "kept", "since": "4b825dc642cb6eb9a060e54bf8d69288fbee4904" }),
                )
                .to_string(),
            ),
            (
                "memstead_health",
                mcp.answered("memstead_health", json!({ "mem": "kept" }))
                    .to_string(),
            ),
            (
                "memstead_overview",
                mcp.text("memstead_overview", json!({ "mem": "kept" })),
            ),
        ];
        drop(mcp);
        out.push(("mem list", json_ok(&fx.view, &["mem", "list"]).to_string()));
        out.push((
            "entity",
            json_ok(&fx.view, &["entity", "kept--one"]).to_string(),
        ));
        out.push((
            "workspace dump",
            json_ok(&fx.view, &["workspace", "dump"]).to_string(),
        ));
        out.push(("health", json_ok(&fx.view, &["health"]).to_string()));
        out.push(("overview", ok(&fx.view, &["overview"])));
        out
    };
    let agreeing = replies(&fx);
    assert!(agreeing.iter().any(|(_, reply)| reply.contains(NEW)));

    set_roster_pin(&fx.view, None);
    assert_eq!(replies(&fx), agreeing, "a row without a pin");

    set_roster_pin(&fx.view, Some(OLD));
    let beside_an_older_row = replies(&fx);
    for ((name, reply), (_, agreed)) in beside_an_older_row.iter().zip(&agreeing) {
        if reply.contains("SCHEMA_PIN_MISMATCH") {
            // The cross-check is the one thing the row adds.
            assert!(
                ["memstead_health", "memstead_overview", "health", "overview"].contains(name),
                "{name}: {reply}"
            );
            continue;
        }
        assert_eq!(reply, agreed, "{name} beside an older row");
    }
}

/// A pin that moves under a running engine is named by the replies of
/// that engine from the next call on: the reading follows the engine's
/// state, not the boot's.
#[test]
fn a_running_engine_names_the_moved_pin_in_every_reply() {
    let fx = store_and_view("write", &["kept"], OLD);
    let mut mcp = Mcp::start(&fx.view);
    let before = mcp.answered("memstead_health", json!({ "mem": "kept" }));
    assert_eq!(before["mem_schemas"][0]["schema"], OLD, "{before}");

    move_pin(&fx.store, "kept", NEW);

    let health = mcp.answered("memstead_health", json!({ "mem": "kept" }));
    assert_eq!(health["mem_schemas"][0]["schema"], NEW, "{health}");
    assert_eq!(health["_mem_schema"], NEW, "{health}");
    let schema = mcp.answered("memstead_schema", json!({ "mem": "kept" }));
    assert_eq!(schema["ref"], NEW, "{schema}");
    let overview = mcp.text("memstead_overview", json!({}));
    assert!(
        overview.contains(&format!("- **Schema:** {NEW}")),
        "{overview}"
    );
    assert!(
        !overview.contains(&format!("- **Schema:** {OLD}")),
        "{overview}"
    );
    // The cross-check a boot raises is raised by the engine that
    // followed, with both values.
    let warned = health["warnings"].as_array().unwrap().iter().any(|w| {
        w["code"] == "SCHEMA_PIN_MISMATCH"
            && w["details"] == json!({ "mem": "kept", "config_pin": NEW, "mount_pin": OLD })
    });
    assert!(warned, "{health}");
}
